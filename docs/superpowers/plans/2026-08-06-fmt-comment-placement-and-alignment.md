# fmt: comment placement and trailing-comment alignment — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make the assembly printer place own-line comments where authors
actually write them and align trailing comments per group, make the `.tmc`
printer stop dropping a single member out of an aligned run, and gate `.tma`
against future drift.

**Architecture:** Core's `format_asm_with` currently prints each item in one
pass, each printer independently padding to a fixed `COMMENT_COL`. A group
column cannot be known until every member of the group has been measured, so
the printer is split into two phases: render each item's code without its
trailing comment, then compute a column per group and emit code + padding +
comment. This mirrors the `.tmc` printer's existing `Rendered { code,
trailing }` shape. Own-line comment placement falls out of the same pass. The
`.tmc` change is a two-line deletion in `trailing_spacing`.

**Tech Stack:** Rust, no new dependencies. `cargo test`, `cargo clippy`,
`cargo fmt`.

**Spec:** `docs/superpowers/specs/2026-08-06-fmt-comment-placement-and-alignment-design.md`
— read it before Task 1. Decisions are labelled D1–D5 there and referenced by
label here.

## Global Constraints

- **PM-1 byte-identity is a standing gate.** Compiled `.pmc` → `.pmo`/`.pmx`
  output must not change. Verify with `cargo test -p mtc-post-machine`.
- **`.pma` formatted output must not change** except for D1's body-comment
  case. Any other `.pma` diff is a bug in the change, not an expected result.
- **The printer is whitespace-only.** No token's text may change. Both
  printers have token-signature guards; they must stay green without being
  relaxed.
- **Formatting is idempotent.** `fmt(fmt(x)) == fmt(x)` for every corpus file.
- **Goldens are derivation-first and must pass WITHOUT regeneration.** Never
  run the `regen` ignored tests in this work. A passing golden run after a
  whitespace-only reformat is the evidence that no token moved.
- **Published docs are forge-agnostic** — no issue/PR numbers or hosting URLs
  in `README.md`, `docs/` pages, or code comments. Cite durable pages as
  `docs/<page>.md (topic keyword)`. This plan and the spec are internal and
  unrestricted.
- **No `docs/superpowers/` path may be cited by new code.**
- **Commits require the maintainer's explicit go-ahead.** Each task ends with
  a commit step; run it only when the maintainer has said to commit. Leave
  changes staged or unstaged otherwise.
- Commit style: conventional with scope — `fix(core):`, `test(turing-machine):`,
  `docs(tmt):`.

---

## File Structure

| File | Responsibility | Tasks |
|---|---|---|
| `crates/core/src/asm/fmt.rs` | Two-phase render, own-line comment column, group column | 1, 2, 3 |
| `crates/turing-machine/src/fmt.rs` | Remove the `LINE_WIDTH` guard from `trailing_spacing` | 4 |
| `docs/examples/brainfuck-utm.tma` | Reformat under the new rules | 5 |
| `crates/turing-machine/tests/fmt_programs.rs` | `.tma` dogfood gate | 6 |
| `docs/formats.md`, `docs/tmt/fmt.md`, `docs/pmt/fmt.md` | Rule statements and worked examples | 7 |

---

### Task 1: Split the assembly printer into measure and emit phases

A pure refactor. Output must stay **byte-identical** for every input. This is
groundwork: D2's group column cannot be computed while each printer is
independently padding to a constant.

**Files:**
- Modify: `crates/core/src/asm/fmt.rs:57-113` (`format_asm_with`), `:160-222`
  (`print_line` / `print_fields`), `:231-247` (`print_routine`), `:251-266`
  (`print_section`), `:268-287` (`print_table_directive`), `:289-355`
  (`print_frame_directive`), `:357-390` (`print_rept`)
- Test: `crates/core/src/asm/fmt.rs` test module (existing tests are the gate)

**Interfaces:**
- Consumes: nothing.
- Produces: `struct Piece { code: String, comment: Option<String>, kind: PieceKind }`
  and `fn render_pieces(cst: &AsmCst, source: &str) -> Vec<Piece>`. Task 2 reads
  `Piece::kind` to find comment items; Task 3 reads `Piece::code` widths to
  compute group columns.

- [ ] **Step 1: Write the byte-identity test**

First capture today's output as a fixture — 400 lines is too much for a string
literal, so it goes in a file:

```bash
cargo build --release
mkdir -p crates/core/src/asm/testdata
cp docs/examples/brainfuck-utm.tma /tmp/bf.tma
./target/release/tmt fmt /tmp/bf.tma
cp /tmp/bf.tma crates/core/src/asm/testdata/flagship_before_refactor.tma
```

Then add to the test module in `crates/core/src/asm/fmt.rs`:

```rust
#[test]
fn refactor_preserves_output_on_the_flagship() {
    // The two-phase split must change nothing. This is a moving
    // baseline: Task 2 regenerates it, Task 3 deletes it once the group
    // column deliberately changes output.
    let src = include_str!("../../../../docs/examples/brainfuck-utm.tma");
    let before = include_str!("testdata/flagship_before_refactor.tma");
    let caps = AsmCaps {
        tables: true,
        rept: true,
        vectors: true,
    };
    assert_eq!(format_asm_with(src, caps).unwrap(), before);
}
```

- [ ] **Step 2: Run it to confirm it passes before any refactor**

Run: `cargo test -p mtc-core --lib asm::fmt::tests::refactor_preserves_output_on_the_flagship`
Expected: PASS. If it fails, the fixture was captured wrong — fix that first.

- [ ] **Step 3: Introduce the Piece type**

```rust
/// What a printed item is, for the group scan in [`comment_columns`].
#[derive(PartialEq)]
enum PieceKind {
    /// A code line that may carry a trailing comment.
    Line,
    /// An own-line comment.
    Comment,
    /// `.section` / `.func` / `.routine` — a structural item.
    Structural,
    /// A `.rept` block; its body prints verbatim.
    Rept,
}

/// One item's rendered code, with its trailing comment held back so a
/// later pass can choose the column.
struct Piece {
    code: String,
    comment: Option<String>,
    kind: PieceKind,
    blank_before: bool,
}
```

- [ ] **Step 4: Convert each printer to return a Piece**

Change each `fn print_X(out: &mut String, …)` to `fn render_X(…) -> Piece`,
dropping its `pad_to(&mut …, COMMENT_COL)` call and putting the comment text
in `Piece::comment` instead. `print_fields` becomes:

```rust
fn render_fields(
    labels: &[LabelCst],
    instr: Option<(&str, &[OperandToken])>,
    trailing: &Option<TrailingComment>,
) -> Piece {
    let mut out = String::new();
    // … unchanged label / mnemonic / operand logic, writing leading
    // own-line labels into `out` and the last line into `cur` …
    out.push_str(cur.trim_end());
    Piece {
        code: out,
        comment: trailing.as_ref().map(|tc| tc.text.clone()),
        kind: PieceKind::Line,
        blank_before: false,
    }
}
```

Note the multi-label case: a non-last label emits its own line inside `code`.
The width that matters for alignment is the **last** line of `code`, not the
whole string — Task 3 measures `code.rsplit('\n').next()`.

- [ ] **Step 5: Add render_pieces**

The item dispatch that `format_asm_with:80-110` does inline moves here, one
arm per `AsmItemKind`, each returning the `Piece` its `render_X` built. The
`blank_before` and `seen_func` bookkeeping comes along unchanged.

```rust
/// One Piece per CST item, comments held back for [`comment_columns`].
fn render_pieces(cst: &AsmCst, source: &str) -> Vec<Piece> {
    cst.items
        .iter()
        .enumerate()
        .map(|(i, item)| {
            let mut p = match &item.kind {
                AsmItemKind::Comment(c) => Piece {
                    code: String::new(),
                    comment: Some(c.text.clone()),
                    kind: PieceKind::Comment,
                    blank_before: false,
                },
                AsmItemKind::Line(l) => render_line(l),
                AsmItemKind::Func(f) => render_func(f),
                AsmItemKind::Section(s) => render_section(s),
                AsmItemKind::RoutineDirective(r) => render_routine(r),
                AsmItemKind::TableDirective(d) => render_table_directive(d),
                AsmItemKind::FrameDirective(d) => render_frame_directive(d),
                AsmItemKind::Rept(r) => render_rept(r, source),
                AsmItemKind::Raw(_) => unreachable!("the structural gate already refused"),
            };
            p.blank_before = i > 0 && item.blank_before;
            p
        })
        .collect()
}
```

`render_func`, `render_section`, and `render_routine` return
`PieceKind::Structural`; `render_rept` returns `PieceKind::Rept`;
`render_line` and `render_table_directive` return `PieceKind::Line`.

A `Comment` piece carries its text in `comment` with an empty `code` — the
emit loop pads from column 0 to the chosen column, which is exactly what an
own-line comment needs.

- [ ] **Step 6: Emit from the pieces, still using the constant**

```rust
let pieces = render_pieces(&cst, source);
let mut out = String::new();
for (i, p) in pieces.iter().enumerate() {
    if i > 0 && p.blank_before {
        out.push('\n');
    }
    let mut line = p.code.clone();
    let mut col = line.rsplit('\n').next().unwrap_or("").chars().count();
    if let Some(c) = &p.comment {
        pad_to(&mut line, &mut col, COMMENT_COL);
        line.push_str(c);
    }
    out.push_str(line.trim_end());
    out.push('\n');
}
```

- [ ] **Step 7: Run the whole core suite**

Run: `cargo test -p mtc-core`
Expected: PASS, including the new byte-identity test and every existing fmt
test. Any failure means the refactor changed output — fix it, do not update
the expectation.

- [ ] **Step 8: Confirm PM and TM are untouched**

Run: `cargo test --workspace`
Expected: PASS.

- [ ] **Step 9: Commit** (only on the maintainer's go-ahead)

```bash
git add crates/core/src/asm/fmt.rs crates/core/src/asm/testdata/flagship_before_refactor.tma
git commit -m "refactor(core): split the assembly printer into measure and emit phases"
```

---

### Task 2: Own-line comment placement (D1)

**Files:**
- Modify: `crates/core/src/asm/fmt.rs:115-136` (`own_line_comment_col`) and
  the emit loop from Task 1
- Modify: `crates/core/src/asm/fmt.rs:633`, `:784-785`, `:836` (the three
  fixtures that pin column 8)

**Interfaces:**
- Consumes: `Piece`, `PieceKind`, `render_pieces` from Task 1.
- Produces: `fn own_line_comment_col(pieces: &[Piece], i: usize, group_col: usize) -> usize`.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_body_comment_prints_at_column_zero() {
    // D1: MNEMONIC_COL leaves comment placement. A comment on its own
    // line inside a .func body is structural, not attached, because the
    // line above it carries no trailing comment to continue.
    let src = ".func f\n        nop\n        ; note\n        ret\n";
    let expected = ".func f\n        nop\n; note\n        ret\n";
    assert_eq!(format_asm(src).unwrap(), expected);
}

#[test]
fn a_comment_run_continues_the_line_above_it() {
    // D1 rule 1: the line above carries a trailing comment, so the run
    // is a continuation and prints at that group's comment column.
    let src = ".func f\n        nop     ; first\n; continued\n        ret\n";
    let expected = format!(
        ".func f\n        nop{pad}; first\n{cont}; continued\n        ret\n",
        pad = " ".repeat(COMMENT_COL - "        nop".len()),
        cont = " ".repeat(COMMENT_COL),
    );
    assert_eq!(format_asm(src).unwrap(), expected);
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p mtc-core --lib asm::fmt::tests::a_body_comment_prints_at_column_zero asm::fmt::tests::a_comment_run_continues_the_line_above_it`
Expected: FAIL. The first gets column 8 instead of 0; the second gets column 8
instead of 32.

- [ ] **Step 3: Rewrite own_line_comment_col**

Write the predicate and the column function together. Task 3 reuses the
predicate for its group scan, so getting it right once matters:

```rust
/// Does the own-line comment at `i` continue a trailing comment above it
/// (docs/formats.md (assembly text)), or open a new structural block? A
/// blank line above breaks the continuation.
fn continues_a_trailing_comment(pieces: &[Piece], i: usize) -> bool {
    if pieces[i].blank_before {
        return false;
    }
    let mut j = i;
    while j > 0 {
        j -= 1;
        match pieces[j].kind {
            PieceKind::Comment if !pieces[j].blank_before => continue,
            PieceKind::Line => return pieces[j].comment.is_some(),
            _ => return false,
        }
    }
    false
}

/// Own-line comment column (docs/formats.md (assembly text)). Two cases:
/// a run continuing the trailing comment above it prints at that group's
/// comment column; everything else is structural and prints at column 0.
///
/// Column 8 is the mnemonic column — where statements live. A comment is
/// not a statement, so it is never placed there.
fn own_line_comment_col(pieces: &[Piece], i: usize, group_col: usize) -> usize {
    if continues_a_trailing_comment(pieces, i) {
        group_col
    } else {
        TOP_COL
    }
}
```

Until Task 3 lands, the emit loop passes `COMMENT_COL` as `group_col`.

- [ ] **Step 4: Run the two tests**

Run: `cargo test -p mtc-core --lib asm::fmt::tests::a_body_comment_prints_at_column_zero asm::fmt::tests::a_comment_run_continues_the_line_above_it`
Expected: PASS.

- [ ] **Step 5: Update the three fixtures that pinned column 8**

These pin the behaviour D1 deliberately removes. Update the *expectation*,
not the input:

- `:633` — `".func f\n        nop\n        ; inside f\n        ret\n"` becomes
  the expected output `".func f\n        nop\n; inside f\n        ret\n"`.
- `:784-785` — expected `"        ; note"` becomes `"; note"`.
- `:836` — same shape as `:633`.

Leave `:632`, `:816`, `:842` alone — they are already structural and stay green.

- [ ] **Step 6: Run the core suite**

Run: `cargo test -p mtc-core`
Expected: PASS. The Task 1 byte-identity fixture will now fail — the flagship's
comments moved. Regenerate that fixture from the new output and note in a
comment that it is a moving baseline until Task 3.

- [ ] **Step 7: Commit** (only on the maintainer's go-ahead)

```bash
git add crates/core/src/asm/fmt.rs crates/core/src/asm/testdata/flagship_before_refactor.tma
git commit -m "fix(core): place own-line assembly comments at column 0 or the continued column"
```

---

### Task 3: Group-wide trailing-comment column (D2)

**Files:**
- Modify: `crates/core/src/asm/fmt.rs` — add `comment_columns`, use it in the
  emit loop
- Modify: `crates/core/src/asm/fmt.rs:592-616`, `:934`, `:979` (tests that
  compute expectations from `COMMENT_COL` directly)
- Delete: the Task 1 byte-identity test and its fixture

**Interfaces:**
- Consumes: `Piece`, `PieceKind` from Task 1; `own_line_comment_col` from Task 2.
- Produces: `fn comment_columns(pieces: &[Piece]) -> Vec<usize>` — one column
  per piece index.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_group_widens_past_the_floor_for_its_widest_member() {
    // D2: column = max(COMMENT_COL, widest code width in group + 1).
    // "        .targets aaaaaaaaaaaaaaaaaaaaaaaaa" is 41 chars, so the
    // group aligns at 42 and the narrow line follows it there.
    let src = ".func f\n        nop     ; short\n        .targets aaaaaaaaaaaaaaaaaaaaaaaaa ; wide\n";
    let out = format_asm_with(src, AsmCaps { tables: true, ..AsmCaps::default() }).unwrap();
    let cols: Vec<usize> = out
        .lines()
        .filter(|l| l.contains(';'))
        .map(|l| l.find(';').unwrap())
        .collect();
    assert_eq!(cols, vec![42, 42], "both members align at the group column");
}

#[test]
fn a_blank_line_starts_a_new_group() {
    let src = ".func f\n        nop     ; a\n\n        .targets aaaaaaaaaaaaaaaaaaaaaaaaa ; b\n";
    let out = format_asm_with(src, AsmCaps { tables: true, ..AsmCaps::default() }).unwrap();
    let cols: Vec<usize> = out
        .lines()
        .filter(|l| l.contains(';'))
        .map(|l| l.find(';').unwrap())
        .collect();
    assert_eq!(cols, vec![32, 42], "the blank line splits them into two groups");
}

#[test]
fn an_uncommented_line_contributes_no_width() {
    let src = ".func f\n        nop     ; a\n        .targets aaaaaaaaaaaaaaaaaaaaaaaaa\n        ret     ; b\n";
    let out = format_asm_with(src, AsmCaps { tables: true, ..AsmCaps::default() }).unwrap();
    let cols: Vec<usize> = out
        .lines()
        .filter(|l| l.contains(';'))
        .map(|l| l.find(';').unwrap())
        .collect();
    assert_eq!(cols, vec![32, 32], "the long uncommented line does not widen the group");
}

#[test]
fn the_group_column_is_never_capped_by_line_width() {
    // D2: unbounded, matching D4. line-too-long reports the result.
    let wide = "a".repeat(70);
    let src = format!(".func f\n        nop     ; a\n        .targets {wide} ; b\n");
    let out = format_asm_with(&src, AsmCaps { tables: true, ..AsmCaps::default() }).unwrap();
    assert!(
        out.lines().any(|l| l.len() > 80),
        "alignment wins over the 80-column limit"
    );
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `cargo test -p mtc-core --lib asm::fmt::tests::a_group_widens asm::fmt::tests::a_blank_line_starts asm::fmt::tests::an_uncommented_line asm::fmt::tests::the_group_column_is_never_capped`
Expected: FAIL — every column comes back 32 or one-space-past-field.

- [ ] **Step 3: Implement comment_columns**

```rust
/// Per-group trailing-comment column (docs/formats.md (assembly text)):
/// `max(COMMENT_COL, widest code width in the group + 1)`. COMMENT_COL is
/// a floor, so a group only ever widens — which is what keeps output
/// unchanged for dialects whose operands all fit before it.
///
/// A group ends at a blank line, an own-line comment at column 0, a
/// structural directive, or a `.rept` block. A `.rept` body prints
/// verbatim rather than through this grid, so it contributes no width:
/// aligning a group to a member that never joins it would be incoherent.
///
/// The column is NOT capped by any line limit; `line-too-long` reports an
/// overlong result. The `.tmc` printer makes the same call.
fn comment_columns(pieces: &[Piece]) -> Vec<usize> {
    let mut cols = vec![COMMENT_COL; pieces.len()];
    let mut start = 0;
    for i in 0..=pieces.len() {
        let ends = i == pieces.len()
            || pieces[i].blank_before
            || matches!(pieces[i].kind, PieceKind::Structural | PieceKind::Rept);
        if !ends {
            continue;
        }
        let widest = (start..i)
            .filter(|&k| pieces[k].comment.is_some() && pieces[k].kind == PieceKind::Line)
            .map(|k| pieces[k].code.rsplit('\n').next().unwrap_or("").chars().count())
            .max()
            .unwrap_or(0);
        let col = COMMENT_COL.max(widest + 1);
        for c in cols.iter_mut().take(i).skip(start) {
            *c = col;
        }
        start = i;
    }
    cols
}
```

A column-0 own-line comment also ends a group, and a continuation run does
not. Reuse `continues_a_trailing_comment` from Task 2 rather than writing a
second predicate — if the group scan and the column choice ever disagree, a
comment aligns to a group it was not counted in. The `ends` predicate becomes:

```rust
let ends = i == pieces.len()
    || pieces[i].blank_before
    || matches!(pieces[i].kind, PieceKind::Structural | PieceKind::Rept)
    || (pieces[i].kind == PieceKind::Comment && !continues_a_trailing_comment(pieces, i));
```

`own_line_comment_col` needs no change from Task 2; it already reads the same
predicate. Only its `group_col` argument changes, from the constant to
`cols[i]`, in Step 4.

- [ ] **Step 4: Use the columns in the emit loop**

Replace the `pad_to(&mut line, &mut col, COMMENT_COL)` in the emit loop from
Task 1 Step 5 with `pad_to(&mut line, &mut col, cols[i])`, and pass `cols[i]`
as `group_col` to `own_line_comment_col`.

- [ ] **Step 5: Run the four tests**

Run: `cargo test -p mtc-core --lib asm::fmt::tests`
Expected: PASS.

- [ ] **Step 6: Update the tests that hardcoded COMMENT_COL**

`:592-616`, `:934`, `:979` compute expectations as `" ".repeat(COMMENT_COL - n)`.
Each is a single-item or narrow group, so its column is still 32 and the
expectation holds — but confirm rather than assume. If one is now in a widened
group, update it and say why in a comment.

- [ ] **Step 7: Delete the Task 1 byte-identity test and its fixture**

Output now deliberately differs. Remove the test and
`crates/core/src/asm/testdata/flagship_before_refactor.tma`.

- [ ] **Step 8: Verify `.pma` output did not move**

```bash
cargo test -p mtc-post-machine
```
Expected: PASS. If any `.pma` fmt test fails other than through D1's
body-comment case, the floor is not doing its job — stop and re-check
`comment_columns` before continuing.

- [ ] **Step 9: Commit** (only on the maintainer's go-ahead)

```bash
git add crates/core/src/asm/fmt.rs
git rm crates/core/src/asm/testdata/flagship_before_refactor.tma
git commit -m "fix(core): align assembly trailing comments per group, not to a fixed column"
```

---

### Task 4: `.tmc` runs always align (D4)

**Files:**
- Modify: `crates/turing-machine/src/fmt.rs:235-276` (`trailing_spacing`),
  and the module doc at `:104-110` and `:230-234`

**Interfaces:**
- Consumes: nothing from Tasks 1–3. Independent; may be done in parallel.
- Produces: no new API.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_run_aligns_even_when_a_member_then_crosses_eighty() {
    // D4: alignment wins; line-too-long reports the result. Before this,
    // the long member kept a single space and dropped out of the run.
    let src = concat!(
        "machine {\n",
        "  tape prog: ops; // brainfuck source + 'H'; the head IS the instruction pointer\n",
        "  tape cnt:  levels; // unary stack of bracket-nesting levels\n",
        "}\n",
    );
    let out = format(src).unwrap();
    let cols: Vec<usize> = out
        .lines()
        .filter(|l| l.contains("//"))
        .map(|l| l.find("//").unwrap())
        .collect();
    assert_eq!(cols[0], cols[1], "both members share the run's column");
    assert!(out.lines().any(|l| l.chars().count() > 80));
}
```

- [ ] **Step 2: Run it to verify it fails**

Run: `cargo test -p mtc-turing-machine --lib fmt::tests::a_run_aligns_even_when_a_member_then_crosses_eighty`
Expected: FAIL — `cols[0]` is 18, `cols[1]` is 21.

- [ ] **Step 3: Delete the guard**

At `:266-270`, replace:

```rust
spacing[k] = if align_col + comment <= LINE_WIDTH {
    align_col - width
} else {
    1
};
```

with:

```rust
spacing[k] = align_col - width;
```

The `comment` binding at `:257-265` becomes unused — delete it too, or clippy
will fail the build.

- [ ] **Step 4: Update the doc comment at `:230-234`**

Replace "except for a member that would then cross the line limit, which keeps
its single space" with a statement that every member aligns and an overlong
result stays reported by `line-too-long`.

- [ ] **Step 5: Run the test and the crate suite**

Run: `cargo test -p mtc-turing-machine`
Expected: the new test PASSES. `fmt_tmc`'s
`every_tmc_source_is_already_fmt_clean` will now FAIL if any corpus file has a
run this changes — that is Task 5's work, not a defect here.

- [ ] **Step 6: Record exactly which corpus files moved**

```bash
cargo build --release
./target/release/tmt fmt --check crates/turing-machine/tests/golden/*.tmc \
    crates/turing-machine/src/stdlib/std.tmc docs/examples/brainfuck-utm.tmc
```
Write the list into the task's completion note. The spec flags this as
unmeasured; if it is more than the flagship's tape block, say so before
proceeding.

- [ ] **Step 7: Commit** (only on the maintainer's go-ahead)

```bash
git add crates/turing-machine/src/fmt.rs
git commit -m "fix(turing-machine): keep every member of an aligned comment run in the run"
```

---

### Task 5: Reformat the corpora

**Files:**
- Modify: `docs/examples/brainfuck-utm.tma`
- Modify: whichever `.tmc` files Task 4 Step 6 listed

**Interfaces:**
- Consumes: the printers from Tasks 3 and 4.
- Produces: a corpus that is fmt-clean, which Task 6 gates.

- [ ] **Step 1: Reformat**

```bash
cargo build --release
./target/release/tmt fmt docs/examples/brainfuck-utm.tma
./target/release/tmt fmt crates/turing-machine/tests/golden/*.tmc \
    crates/turing-machine/src/stdlib/std.tmc docs/examples/brainfuck-utm.tmc
```

Formatting the whole `.tmc` corpus is safe — every file is already clean per
`#66`'s gate, so only the ones Task 4 Step 6 listed can move. `git status`
afterwards must name exactly that list and nothing else; an unexpected file
means D4 reached further than measured, which is worth stopping for.

- [ ] **Step 2: Read the `.tma` diff and check it against the spec**

```bash
git diff docs/examples/brainfuck-utm.tma
```

Confirm against the spec's Expected output section: the 20-line file header
and all ten `; ==== … ====` banners at column 0; the `; no catch-all` block
aligned under `; 'H'`; the tables at 32 with `Dsbp`'s group at 33; the five
`.rept` body comments unchanged at 40. Anything else is a bug — go back to
Task 2 or 3.

- [ ] **Step 3: Prove no token moved**

```bash
cargo test -p mtc-turing-machine --test golden_programs --test tmc_golden --test stdlib_golden
```
Expected: PASS **without regeneration**. `golden_programs` compiles and runs
both the `.tma` and `.tmc` flagships against derived snapshots, so this is the
real evidence the reformat was whitespace-only. If a golden fails, the printer
changed a token — do not regenerate.

- [ ] **Step 4: Run everything**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS.

- [ ] **Step 5: Commit** (only on the maintainer's go-ahead)

```bash
git add docs/examples/brainfuck-utm.tma crates/turing-machine/tests/golden crates/turing-machine/src/stdlib
git commit -m "fmt: reformat the .tma flagship and the .tmc runs the new rules move"
```

---

### Task 6: `.tma` fmt dogfood gate

The original ask on #68. It goes last because it pins whatever canonical form
is in force.

**Files:**
- Modify: `crates/turing-machine/tests/fmt_programs.rs`

**Interfaces:**
- Consumes: `fmt_tma` (already defined at `:34`) and the reformatted corpus
  from Task 5.
- Produces: no new API.

- [ ] **Step 1: Write the gate**

```rust
/// The `.tma` dogfood lock, mirroring `fmt_tmc.rs`'s
/// `every_tmc_source_is_already_fmt_clean`: every `.tma` source the
/// repository ships must already be in canonical form, so formatting it
/// is a byte-for-byte no-op. Any future printer change that would
/// reformat a shipped source fails here first.
#[test]
fn every_tma_source_is_already_fmt_clean() {
    let path =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/examples/brainfuck-utm.tma");
    let src = fs::read_to_string(&path).expect("read brainfuck-utm.tma");
    assert_eq!(fmt_tma(&src), src, "brainfuck-utm.tma is not fmt-clean");
}
```

- [ ] **Step 2: Run it**

Run: `cargo test -p mtc-turing-machine --test fmt_programs`
Expected: PASS, because Task 5 reformatted the file. If it fails, Task 5 was
incomplete — go back rather than editing the file by hand.

- [ ] **Step 3: Prove the gate can fail**

Temporarily add a stray space to a line in `docs/examples/brainfuck-utm.tma`,
re-run, confirm FAIL, then revert. A gate never seen red is not known to work.

- [ ] **Step 4: Commit** (only on the maintainer's go-ahead)

```bash
git add crates/turing-machine/tests/fmt_programs.rs
git commit -m "test(turing-machine): dogfood-gate the .tma corpus for fmt-cleanliness"
```

---

### Task 7: Documentation

**Files:**
- Modify: `docs/formats.md:366-375` (assembly text — the grid and the
  own-line comment rule)
- Modify: `docs/tmt/fmt.md:148-153`, `:330-338` (the line-length statement)
- Verify: worked examples in `docs/pmt/fmt.md` and `docs/tmt/fmt.md`

**Interfaces:**
- Consumes: the finished behaviour from Tasks 2–4.
- Produces: nothing code-facing.

- [ ] **Step 1: Rewrite the grid paragraph in `docs/formats.md`**

It currently reads "trailing comments at column 32". State instead that
trailing comments align per group at `max(32, widest code width in the group +
1)`, that a group ends at a blank line, a column-0 comment, a structural
directive, or a `.rept` block, that uncommented lines and `.rept` bodies
contribute no width, and that the column is not capped by the 80-column limit.

- [ ] **Step 2: Add the own-line comment rule to `docs/formats.md`**

Two cases: a run continuing the line above prints at that group's comment
column; everything else prints at column 0. Say explicitly that column 8 is
the mnemonic column and no comment is placed there.

- [ ] **Step 3: Strengthen the line-length statement in `docs/tmt/fmt.md`**

`:335` says fmt does not rewrap an overlong line. It must now also say that
alignment can *lengthen* a line past 80, and that `line-too-long` reports the
result.

- [ ] **Step 4: Re-run every worked transcript in both fmt pages**

Both pages quote real tool output. Re-run each command and paste the result
rather than eyeballing it. `docs/pmt/fmt.md`'s `.pma` example should be
unchanged — if it is not, the floor failed and Task 3 needs revisiting.

- [ ] **Step 5: Check for forge references**

```bash
grep -nE '#[0-9]+|github\.com' docs/formats.md docs/tmt/fmt.md docs/pmt/fmt.md
```
Expected: no output. Published docs are forge-agnostic.

- [ ] **Step 6: Full gate**

Run: `cargo test --workspace && cargo clippy --workspace --all-targets -- -D warnings && cargo fmt --check`
Expected: PASS.

- [ ] **Step 7: Commit** (only on the maintainer's go-ahead)

```bash
git add docs/formats.md docs/tmt/fmt.md docs/pmt/fmt.md
git commit -m "docs: state the per-group comment column and own-line comment rule"
```

---

## Closing checklist

- [ ] `cargo test --workspace` green
- [ ] `cargo clippy --workspace --all-targets -- -D warnings` clean
- [ ] `cargo fmt --check` clean
- [ ] Goldens passed without regeneration at every step
- [ ] `crates/post-machine` compiled output unchanged
- [ ] `.pma` formatted output unchanged except D1's body-comment case
- [ ] The `.tma` gate has been seen to fail on a deliberate defect
- [ ] `#68` and `#69` closed with what shipped, and with the corrections to
      their original evidence recorded
