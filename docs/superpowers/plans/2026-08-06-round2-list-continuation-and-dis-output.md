# Round 2: list continuation and disassembler output — Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close the blocking regression the whole-branch review found, make `tmt dis` output valid assembly, and teach the `.tma` dialect to wrap its three unbounded lists — all before the release cut, while the dialect version is still free to move.

**Architecture:** The `.tma` CST is line-oriented by construction (`parse_asm_cst_with` splits on `source.lines()`). List continuation is therefore a pre-pass that joins lines ending in a comma into one logical line, leaving the CST's shape untouched. The disassembler changes are independent of that and mostly about emitting what a person would write: blank lines between tables, and label names that are actually defined.

**Tech Stack:** Rust, no new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-06-fmt-comment-placement-and-alignment-design.md`, decisions D6–D9 and the "Corrections from the whole-branch review" section. Read it before Task 1.

**Issues:** #70 (dis output cannot be reassembled), #71 (no way to wrap a `.targets` list).

## Global Constraints

- **PM-1 is not touched.** All three wrapped lists are gated behind `AsmCaps::tables`, which `pm1_syntax()` never enables. Any `crates/post-machine` diff, or any change to `.pma` formatted output, is a defect in the change.
- **`TM1_TMA_DIALECT_VERSION` stays at `0.3`.** No bump — that version has never shipped.
- **The printer stays whitespace-only** for existing input, and idempotent: `fmt(fmt(x)) == fmt(x)`.
- **Goldens are derivation-first and must never be regenerated.** Never run the `#[ignore]`d `regen` tests.
- **Published docs and code comments are forge-agnostic** — no issue/PR numbers, no hosting URLs. Cite durable pages as `docs/<page>.md (topic keyword)`. Nothing may cite `docs/superpowers/`.
- **Never name a spec decision label (`D6`, `D7`, …) in a code comment.** State the rule in prose. This leaked ten times in round 1.
- **Run every command in the foreground**, one `cargo` at a time. Two agents in round 1 stalled on background jobs; one produced false failures from concurrent runs.
- Conventional commits with scope. No Claude/Claude Code attribution — hard rule.
- Commits require the maintainer's go-ahead, which was granted for this branch. Do not push, do not merge.

---

## File Structure

| File | Responsibility | Task |
|---|---|---|
| `crates/core/src/asm/fmt.rs` | I1/I2/M1 corrections; wrapping in the printer | 1, 3 |
| `crates/core/src/asm/cst.rs` | continuation pre-pass | 2 |
| `crates/core/src/asm/disassembler.rs` | blank lines, label unification, wrapped emit | 4 |
| `crates/turing-machine/tests/fmt_programs.rs` | the dis-canonical gate | 5 |
| `docs/formats.md`, `docs/tmt/fmt.md` | the continuation rule and the corrections | 1, 5 |

---

### Task 1: Whole-branch review corrections

Independent of everything else. Clears the deck before the structural work.

**Files:**
- Modify: `crates/core/src/asm/fmt.rs` (I1, I2, M1)
- Modify: `docs/formats.md` (I1 or I2, depending on which way each is resolved)

**Interfaces:** none produced or consumed.

- [ ] **Step 1: Decide each of I1 and I2 — code or doc**

Both are mismatches between what the code does and what `docs/formats.md` says, written in the same round. Each can be fixed in either place. Read the spec's rationale for the group column, then pick the side that makes the rule simpler to state, and say why in the report.

- **I1** — `render_rept` bakes the `.rept` header's trailing comment at the fixed `COMMENT_COL`, while `.endr`'s goes through the group column like everything else. Two mechanisms doing one job.
- **I2** — `comment_columns`'s width filter is `comment.is_some() && kind == PieceKind::Line`, so a `.section`/`.func`/`.routine` line *with* a comment is padded to the group column but contributes no width to it.

- [ ] **Step 2: Write a failing test for each**

For I2, this test currently fails — the `.func` line's comment lands at 43 while the narrow lines sit at 32:

```rust
#[test]
fn a_commented_structural_directive_widens_its_group() {
    let src = ".func aFunctionWithAnExtremelyLongNameHere ; what it does\n        rd      ; read\n        stp     ; done\n";
    let out = format_asm(src).unwrap();
    let cols: Vec<usize> = out.lines().filter(|l| l.contains(';')).map(|l| l.find(';').unwrap()).collect();
    assert!(cols.windows(2).all(|w| w[0] == w[1]), "all three share one column, got {cols:?}");
}
```

Write the I1 analogue the same way, asserting the `.rept` header's comment shares its group's column.

If you resolve either the other way — doc changes to match code — write the test to pin the documented behaviour instead, and say so.

- [ ] **Step 3: Run both, confirm they fail**

Run: `cargo test -p mtc-core --lib asm::fmt::`
Expected: the two new tests FAIL with concrete column mismatches. Record the actual numbers.

- [ ] **Step 4: Fix M1 — the vacuous test**

`the_group_column_is_never_capped_by_line_width` asserts `out.lines().any(|l| l.len() > 80)`. Its fixture's `.targets` line is already 87 characters of code before any comment, so that holds under a fixed-column printer too — the test would pass unchanged before this branch. Replace the assertion with one on the *narrow* line's comment column, which is what "uncapped" actually means. Derive the expected number from real output, do not guess.

- [ ] **Step 5: Implement I1 and I2 as decided**

- [ ] **Step 6: Verify**

Run, in the foreground and one at a time: `cargo test -p mtc-core`, then `cargo test --workspace`, then `cargo clippy --workspace --all-targets -- -D warnings`, then `cargo fmt --check`.

Confirm `crates/post-machine` shows no diff and no `.pma` test moved.

- [ ] **Step 7: Update `docs/formats.md` if the resolution needs it**

- [ ] **Step 8: Commit**

```bash
git add crates/core/src/asm/fmt.rs docs/formats.md
git commit -m "fix(core): route .rept header comments through the group column and count commented directives in its width"
```

---

### Task 2: List continuation in the CST

The grammar half of #71. No printer change yet — this task only makes the assembler *accept* a wrapped list.

**Files:**
- Modify: `crates/core/src/asm/cst.rs` (the pre-pass, ahead of the `source.lines()` loop)

**Interfaces:**
- Produces: a CST in which a `.targets` / `.exits` / `.map` directive may have originated from several physical lines. Downstream consumers see one logical line. Task 3's printer and Task 4's disassembler both rely on this.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn a_trailing_comma_continues_a_targets_list() {
    let src = ".section tables\nD0:     .targets aa,\n        bb\n.section code\n.func main\naa:     rd\n        stp\nbb:     stp\n";
    let caps = AsmCaps { tables: true, ..AsmCaps::default() };
    let cst = parse_asm_cst_with(src, caps);
    // The two physical lines must lower to ONE table directive carrying
    // both labels — not two items, and not a Raw line.
    let targets: Vec<_> = cst.items.iter().filter(|i| matches!(i.kind, AsmItemKind::TableDirective(_))).collect();
    assert_eq!(targets.len(), 1, "one logical directive, got {}", targets.len());
}
```

Write the `.exits` and `.map` analogues. Then an end-to-end one asserting the whole thing assembles:

```rust
#[test]
fn a_wrapped_targets_list_assembles() {
    let src = ".section tables\nD0:     .targets aa,\n        bb\n.section code\n.func main\naa:     rd\n        stp\nbb:     stp\n";
    assert!(assemble(src, tm1_syntax()).is_ok());
}
```

- [ ] **Step 2: Run them, confirm they fail**

Run: `cargo test -p mtc-core --lib asm::cst::`
Expected: FAIL. Today the continuation line is a separate item and the assembler reports `dispatch targets are label names [bad-table]`.

- [ ] **Step 3: Implement the pre-pass**

Ahead of `for (idx, line) in source.lines().enumerate()` in `parse_asm_cst_with`, fold physical lines into logical ones: a line whose last non-whitespace, non-comment character is `,` joins the following line. Apply it ONLY when the line's directive word is `.targets`, `.exits`, or `.map` — a trailing comma elsewhere stays the error it is today.

The CST is LOSSLESS. A joined line must still be able to reproduce its original text exactly, so retain the physical line boundaries and each segment's indentation in the CST node. Whatever representation you choose, the round-trip test in Step 5 is the arbiter.

Line numbers must continue to refer to PHYSICAL lines — diagnostics, `-g`'s debug line map, and both LSP services all read them, and a diagnostic pointing at the wrong line is worse than the bug being fixed.

- [ ] **Step 4: Run the new tests**

Run: `cargo test -p mtc-core --lib asm::cst::`
Expected: PASS.

- [ ] **Step 5: Prove losslessness and diagnostics**

Add a test asserting a wrapped list round-trips through the CST byte-for-byte, and one asserting a syntax error on the *second* physical line of a wrapped list reports that line's number, not the first's.

- [ ] **Step 6: Full gate**

Run, one at a time: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`.

`crates/post-machine` must show no diff. `.pma` cannot reach this path — the pre-pass keys on `tables`-capped directive words — but confirm rather than assume.

- [ ] **Step 7: Commit**

```bash
git add crates/core/src/asm/cst.rs
git commit -m "feat(core): accept a trailing comma as a list continuation in .targets, .exits, and .map"
```

---

### Task 3: The printer wraps

**Files:**
- Modify: `crates/core/src/asm/fmt.rs`

**Interfaces:**
- Consumes: Task 2's continuation-capable CST.
- Produces: wrapped output that Task 5's gate will pin.

- [ ] **Step 1: Write the failing test**

```rust
#[test]
fn a_long_targets_list_wraps_at_eighty_columns() {
    let names: Vec<String> = (0..40).map(|i| format!("label_number_{i:02}")).collect();
    let src = format!(".section tables\nD0:     .targets {}\n", names.join(", "));
    let out = format_asm_with(&src, AsmCaps { tables: true, ..AsmCaps::default() }).unwrap();
    assert!(out.lines().all(|l| l.chars().count() <= 80), "every line fits: {:?}", out.lines().map(|l| l.chars().count()).max());
    assert!(out.lines().count() > 2, "the list actually wrapped");
}
```

- [ ] **Step 2: Run it, confirm it fails**

Run: `cargo test -p mtc-core --lib asm::fmt::a_long_targets_list_wraps`
Expected: FAIL — one line, far over 80.

- [ ] **Step 3: Implement wrapping**

Wrap when the line would exceed 80 columns. Continuation lines align under the first element. Note the printer's module doc currently states there is "no comma-group wrapping, no line-width budget" — that sentence is now false and must be rewritten in the same commit.

- [ ] **Step 4: Prove idempotence on wrapped output**

The formatter must be a fixed point on its own wrapped output. Add a test formatting twice and comparing, and confirm the existing corpus-wide idempotence tests still pass.

- [ ] **Step 5: Full gate, foreground, one at a time**

`cargo test --workspace`, clippy, `cargo fmt --check`. The `.tma` and `.tmc` dogfood gates must stay green — no corpus file should move, since none currently has a list long enough to wrap. If one moves, report it rather than committing it.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/asm/fmt.rs
git commit -m "feat(core): wrap long .targets, .exits, and .map lists at the line limit"
```

---

### Task 4: Disassembler output

Three changes, all to the same file, all about emitting what a person would write. Closes #70 and the blocking regression.

**Files:**
- Modify: `crates/core/src/asm/disassembler.rs`

**Interfaces:**
- Consumes: Task 2's grammar and Task 3's wrapping.
- Produces: assemblable, fmt-clean disassembler output that Task E gates.

- [ ] **Step 1: Write the failing tests**

```rust
#[test]
fn dis_output_assembles_without_a_debug_map() {
    // Round-trip: assemble the flagship, disassemble, reassemble.
    // Fails today with `dispatch targets are label names [bad-table]`.
}

#[test]
fn dis_output_assembles_with_a_debug_map() {
    // Same, with -g. Fails today with `unknown table label`.
}

#[test]
fn dis_separates_tables_with_blank_lines() {
    // Every `T<n>:` table after the first is preceded by a blank line.
}
```

Fill each body in against the real API — read the existing tests in that file for how a fixture is assembled and disassembled.

- [ ] **Step 2: Run them, confirm all three fail**

Record the actual errors.

- [ ] **Step 3: Unify the label mechanism**

`.targets` must always emit label names, and every name it emits must be defined at its address in the code section. Prefer a debug-map name when one exists; synthesize one otherwise, using the existing `L<n>` synthesizer extended to cover dispatch-target addresses, not only jump targets.

Delete the `; unresolved dispatch offsets (no debug labels)` comment — the situation it explains no longer occurs.

- [ ] **Step 4: Emit blank lines between tables**

A blank line before each table label after the first. This is what makes the formatter a no-op on the output, verified before the design was adopted: with these blank lines, `tmt fmt` changed 0 lines.

- [ ] **Step 5: Emit wrapped lists**

Use Task 3's wrapping so a wide dispatch table is readable as emitted, not only after a formatting pass.

- [ ] **Step 6: Verify the whole round-trip by hand**

```bash
cargo build --release
./target/release/tmt asm docs/examples/brainfuck-utm.tma -o /tmp/bf.tmo
./target/release/tmt dis /tmp/bf.tmo > /tmp/bf_dis.tma
./target/release/tmt asm /tmp/bf_dis.tma -o /tmp/bf2.tmo   # must succeed
./target/release/tmt fmt --check /tmp/bf_dis.tma           # must exit 0
```

Repeat with `-g`. Paste all of it into the report.

- [ ] **Step 7: Full gate, foreground, one at a time**

- [ ] **Step 8: Commit**

```bash
git add crates/core/src/asm/disassembler.rs
git commit -m "fix(core): emit assemblable, human-shaped disassembly — defined labels, blank lines between tables, wrapped lists"
```

---

### Task 5: The gate and the documentation

**Files:**
- Modify: `crates/turing-machine/tests/fmt_programs.rs`
- Modify: `docs/formats.md`, `docs/tmt/fmt.md`

- [ ] **Step 1: Add the TM dis-canonical gate — as a PAIR of assertions**

Mirror `crates/post-machine/tests/fmt_pma.rs`'s `dis_output_is_already_canonical`, but assert **two** things:

1. `fmt_tma(&dis) == dis` — the output is already canonical.
2. `assemble(&dis, tm1_syntax())` succeeds — the output is valid assembly.

**Scope the gate to OBJECT disassembly (`.tmo`).** Executable (`.tmx`) disassembly neither reassembles nor formats clean; that is tracked as its own issue and is deliberately out of scope here.

Why a pair, measured rather than assumed: Task 4 made three changes, and format-identity alone detects only one. Reverting the blank-line emission leaves `tmt fmt --check` at rc=0. Reverting the code-label definitions also leaves it at rc=0, even though reassembly then fails with `unknown table label`. Only the un-wrap defect gives rc=1. A single format-identity assertion would ship two thirds of Task 4 ungated at the integration level.

This is the test whose absence let the blocking regression through — PM had it, TM did not.

- [ ] **Step 2: Prove BOTH assertions can fail**

A gate never observed failing is not known to work — and this round produced four tests that passed for reasons unrelated to what they claimed to pin. Verify each half against the defect that actually trips it:

- **Format identity:** revert Task 4's list *wrapping* so the disassembler emits an unwrapped list. Confirm the gate fails. Do NOT use the blank-line revert here — it was measured on the shipped tool and gives rc=0, so it would tell you a working gate is broken.
- **Assemblability:** revert Task 4's code-label definitions so `.targets` names something nothing defines. Confirm the gate fails with `unknown table label`.

Restore after each, confirm green, and record both transcripts.

- [ ] **Step 3: Document the continuation rule**

`docs/formats.md` (assembly text): a trailing comma continues `.targets`, `.exits`, and `.map` onto the next line; the formatter wraps them at the line limit; the other list-shaped operands are bounded by tape count and do not wrap. State that the dialect version does not move, and why.

- [ ] **Step 4: Document the disassembler's output shape**

That `tmt dis` emits assemblable assembly, with defined labels, blank lines between tables, and wrapped lists.

- [ ] **Step 5: Re-run every touched transcript**

Both fmt pages carry real tool output. Re-run and paste; never hand-edit to look right.

- [ ] **Step 6: Forge-reference and label sweeps**

```bash
grep -nE '#[0-9]+|github\.com' docs/formats.md docs/tmt/fmt.md docs/pmt/fmt.md
grep -rnE '\bD[0-9]\b' crates/ --include="*.rs"
```
First: only `trap #0` / `retx #1` operand syntax. Second: only dispatch-table labels, never a spec decision label.

- [ ] **Step 7: Full gate and commit**

---

## Closing checklist

- [ ] `cargo test --workspace` green
- [ ] clippy and `cargo fmt --check` clean
- [ ] Goldens passed without regeneration throughout
- [ ] `crates/post-machine` zero diff; `.pma` formatted output unmoved
- [ ] `TM1_TMA_DIALECT_VERSION` still `0.3`
- [ ] `tmt dis` output assembles, both with and without `-g`
- [ ] `tmt fmt --check` on `tmt dis` output exits 0
- [ ] The dis-canonical gate has been seen to fail on a deliberate defect
- [ ] #70 and #71 closed with what shipped

---

### Task 6: The executable disassembly path

Closes the executable-path issue. The object path was brought to
round-trip parity in Task 4; `disassemble_executable` has its own table
loop and was untouched, so it carries three separable defects. Measured
across 7 golden programs × 3 call mechanisms × with and without `-g` —
42 executables.

**Files:**
- Modify: `crates/core/src/asm/disassembler.rs`

**Interfaces:** consumes Task 3's `wrap_operand_list` (already
`pub(super)`) and Task 4's `synthesized_label` / `code_label` /
`table_code_labels`. Reuse them — do not write a second copy for the
executable path. A second copy is what produced these defects.

- [ ] **Step 1: Write three failing tests, one per defect**

Each must fail today for the stated reason, not incidentally:

1. `dis_of_a_linked_image_without_map_labels_assembles` — link a
   non-`-g` object, disassemble, reassemble. Fails today with
   `dispatch targets are label names [bad-table]`, because
   `render_linked_dispatch_table` emits raw offsets whenever the map
   carries no labels. That is the DEFAULT case: `tmt link` on a
   non-`-g` object writes `"labels": []`, so `--map` does not help.
   24 of the 42 combinations are unassemblable for this reason alone.
2. `dis_of_a_linked_image_is_fmt_clean` — with `-g` throughout,
   `fmt_tma(&dis) == dis`. Fails today: a 1048-character `.targets`
   and no blank lines between tables, because neither the wrapping nor
   the separator reaches this loop.
3. `dis_of_a_linked_image_emits_routine_signatures` — the
   `a5_call_across_alphabets` golden fails `bad-signature` even with
   `-g`, because the executable path omits `.routine` for library
   functions.

- [ ] **Step 2: Run them, record the three distinct failures**

- [ ] **Step 3: Bring the executable table loop to object-path parity**

Same label synthesizer so every emitted name is defined at its address;
same blank line between tables; same list wrapping.

- [ ] **Step 4: Emit `.routine` for library functions**

- [ ] **Step 5: Verify the full matrix by hand**

All 7 goldens × 3 call mechanisms × ±`-g`: disassemble the linked
image, reassemble it, and run `tmt fmt --check` over it. Paste the
matrix into the report. Any combination that still fails must be named,
not summarised away.

- [ ] **Step 6: Extend Task 5's gate to executables**

Task 5's gate asserts a pair — format identity AND assemblability —
scoped to objects because these defects were known. With them fixed,
widen it to executables and drop the object-only scoping note.

- [ ] **Step 7: Correct the two remaining doc overclaims**

`docs/formats.md` says disassembler output "is always valid assembler
input and round-trips to the original bytes". Once this task lands that
becomes true without qualification — verify before removing the caveat
Task 5 added. `disassembler.rs`'s module doc carries the same claim.
