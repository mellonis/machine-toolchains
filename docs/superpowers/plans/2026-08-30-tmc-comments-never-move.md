# `.tmc` fmt — a comment is never moved

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make `tmt fmt` print every comment between the same two significant
tokens it was written between — so the formatter never relocates a comment — and
state that as a property of the tool.

**Architecture:** The printer currently *buckets* trivia (`open_trailing`,
`interior`, `unclaimed_inside`, the doc run) and flushes each bucket at a
construct boundary. That is why a comment moves: the bucket's flush point, not
the comment's position, decides where it lands. The change is to emit a comment
at its real position in the token stream and let the surrounding layout absorb
it. The buckets do not disappear — they still decide *layout* (does this list go
multi-line?) — but they stop deciding *placement*.

**Tech Stack:** Rust (workspace pinned by `rust-toolchain.toml`),
`crates/turing-machine/src/fmt/{print,trivia}.rs`, `mtc-core`'s `syntax`
framework.

**Spec:** none. This plan's evidence base is the measured audit in
`docs/superpowers/specs/2026-08-30-tmc-comment-audit.md` (Task 0 writes it from
the scratchpad harness). The target rule was chosen by the maintainer from three
options, with the trade-offs measured.

---

## The rule this plan implements

> A comment is printed between the same two significant tokens it was written
> between. The formatter may change the whitespace around it; it may not change
> which tokens it sits between.

`use` already satisfies this today, which is the proof it is implementable at all.

Two consequences that are part of the rule, not exceptions to it:

- **A line comment forces a break.** `alphabet // c` cannot be followed by the
  name on the same line, so the declaration continues on the next. That is a
  whitespace change, which the rule permits.
- **Layout may still change.** A list may go multi-line to accommodate a comment.
  What may not change is the comment's position in the token stream.

---

## Rulings made with the maintainer before execution

**The grid, when a rule carries a comment.** A BLOCK comment keeps its rule on
the shared grid, occupying its column; a LINE comment takes the rule off the
grid, because it forces a break and a broken row cannot be a table row.

```
['b'] -> write ['a'] move [>] goto s;      ['b'] -> write ['a'] move [>] goto s;
['a'] -> /* c */     move [>] goto s;      ['a'] -> // c
['_'] -> stop;                                      move [>] goto s;
```

Accepted cost, stated so nobody re-opens it: a long block comment in one rule
widens that column for every rule in the state's grid group. That is the price of
keeping the table a table, and the maintainer chose it knowing the alternative.

**A line comment in a header follows `use`.** `use` already implements the rule,
and its shape is the model for the other nine families: the remainder moves to
the next line, indented to the construct's continuation indent.

```
use // c
    a::b;
```

**The line limit does not constrain a comment.** Measured on today's printer: a
rule whose comment pushes it to 88 columns is printed at 88 columns. `LINE_WIDTH`
governs list-breaking decisions, not a hard wrap, so never-move introduces no new
conflict with it.

---

## How much of the pinned suite this disturbs — measured, not estimated

Of the `pins(src, expected)` call sites in `crates/turing-machine/src/fmt/print.rs`
carrying a literal source: **61 sites, 44 of which contain a comment, and only 9
of those sit in a slot this plan moves.** The other 35 comment-bearing sources are
in the four groups the audit found already correct (just inside an opener,
trailing a closer, own line before a closer, and every `use` position), so they
must come through UNCHANGED — they are this plan's regression guard, not its work.

That ratio is what makes the individual-justification constraint practical rather
than absurd. **If a change makes far more than ~9 literals move, stop: the change
is broader than the rule requires.**

---

## Global Constraints

- **This plan deliberately CHANGES output, and the thing that made the previous
  plan verifiable is gone.** The `.tmc` formatter's old C1 printer was deleted at
  the cutover, and the 153 `pins(src, expected)` literals in
  `crates/turing-machine/src/fmt/print.rs` encode the OLD, comment-moving
  behaviour, captured mechanically from that printer. There is no second
  implementation to compare against and no oracle that can say the new output is
  right. **Every literal this plan changes must be justified individually, by
  showing the comment sits between the same two significant tokens as in the
  source.** A bulk re-capture of expected output is forbidden: it would make the
  tests agree with whatever the code does, which is exactly the failure the
  previous plan spent nine tasks avoiding.
- **Token preservation and idempotency are the two gates that survive.** After
  every change: re-lex the output and compare the significant-token stream with
  the input's (no token added, dropped or respelled), and confirm
  `format(format(x)) == format(x)`. These are mechanical and cannot be argued
  with; lean on them.
- **The audit is the acceptance criterion.** The scratchpad harness measures 61
  grammatical positions × {block, line} × 3 passes. Task 0 lands it in the repo.
  The plan is done when, for every position it declares in scope, the comment is
  emitted between its original neighbours.
- **`crates/core` and `crates/post-machine` get no diff.** Check with
  `git diff --stat <plan-base> -- crates/core crates/post-machine`, which must
  print nothing. `.pmc` has the same class of behaviour and is explicitly OUT of
  scope; if this plan's approach proves out, PM is a follow-up.
- **The compiled-stdlib byte-identity gate is a NEGATIVE CONTROL here**, not
  proof: it runs the compiler, not the formatter. Run it to prove the change did
  not leak into the compiler; never cite it as evidence about fmt's output.
- Conventional commits with scope. **No AI/Claude attribution in any commit
  message or artifact.**
- Code comments cite durable `docs/` pages by page + parenthetical keyword. Never
  cite a `docs/superpowers/` spec, plan or SDD artifact from code, and never let
  `spec §N` reach a doc comment. Published docs are forge-agnostic.

---

## What the audit measured

61 positions × 2 flavours × 3 passes = 122 runs against `tmt` at `13f5b3d`.

**Already correct — no comment moves, and these must not regress:**

| group | surfaces | result |
|---|---|---|
| just inside an opener | 7 | 7/7 stay |
| trailing a closer | 6 | 6/6 stay |
| own line before a closer | 7 | 7/7 stay |
| `use`, every position | 5 | 5/5 stay |

**In scope — the comment moves:**

| position | current destination | families |
|---|---|---|
| keyword → name | own line in body | `namespace`, `machine`, `state` |
| keyword → name | riding the list's `(` | `routine`, `graph`, `graft`, `bind` |
| keyword → name | trailing the `;` | `tape` |
| keyword → name | inside `{`, inline — **and unstable, settles at pass 2** | `alphabet` |
| name → opener | same destinations as above | all of the above |
| inside a list entry | pushed to the entry's end, ahead of the separator | 5 list surfaces |
| between `->` and an action keyword | into the following vector | rules |

**Facts that bound the work:** no comment is ever lost (0/122), no fixture fails
to parse (0/122), and exactly one shape is non-idempotent today (`alphabet`
header, block flavour).

---

## File Structure

| File | Responsibility |
|---|---|
| `docs/superpowers/specs/2026-08-30-tmc-comment-audit.md` (new, Task 0) | The measured audit, as the evidence base and the acceptance criterion. |
| `crates/turing-machine/tests/comment_positions.rs` (new, Task 0) | The audit as an executable test: every position, both flavours, asserting the comment's significant-token neighbours are unchanged. |
| `crates/turing-machine/src/fmt/trivia.rs` (modified, Tasks 2–5) | Placement stops being a bucket decision. |
| `crates/turing-machine/src/fmt/print.rs` (modified, Tasks 2–5) | Emits comments at position; the `pins` literals change here, individually. |
| `docs/tmt/fmt.md`, `CLAUDE.md` (modified, Task 6) | The relocation roster is replaced by the never-move rule. |

---

## Task 0: land the audit as a test before changing anything

**Files:**
- Create: `crates/turing-machine/tests/comment_positions.rs`
- Create: `docs/superpowers/specs/2026-08-30-tmc-comment-audit.md`

**Interfaces:**
- Produces: the executable acceptance criterion every later task is measured against.

- [ ] **Step 1: Port the harness**

The scratchpad harness (`cases.py`, `run.py`, `group.py`, `inline.py`) enumerates
61 positions with a `@C@` slot. Port it to Rust as a table of
`(id, template)` pairs, substituted with `/* c */` and with `// c`.

- [ ] **Step 2: Write the assertion that matters**

Not "the output equals this string" — that pins today's behaviour, which is what
this plan changes. Assert the INVARIANT instead:

```rust
/// A comment's significant-token neighbours survive formatting. This is the
/// whole rule, mechanically: lex the source and the output with comments
/// retained, find the comment in each, and compare the nearest significant
/// token before and after it. Layout may differ; neighbours may not
/// (docs/tmt/fmt.md (comments are never moved)).
fn neighbours(src: &str) -> (Option<String>, Option<String>) { /* … */ }

#[track_caller]
fn never_moves(src: &str) {
    let out = format(src).expect("formats");
    assert_eq!(neighbours(&out), neighbours(src), "comment moved:\n{src}\n{out}");
}
```

- [ ] **Step 3: Run it and record the baseline**

Expected: the four already-correct groups PASS; the in-scope positions FAIL.
Mark each currently-failing case `#[ignore]` with its measured destination in
the ignore reason, so the file doubles as the work list and un-ignoring one is
how a later task proves its surface.

- [ ] **Step 4: Write the audit doc from the run's output, not from memory**

Every number in it comes from the test run. State the four already-correct groups
as regressions to protect, not as work.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/tests/comment_positions.rs docs/superpowers/specs
git commit -m "test(turing-machine): pin where a comment may and may not move"
```

---

## Task 1: feasibility spike — prove the rule is reachable, or find what is not

**Files:** none committed except a report. This is a spike; its output is an answer.

The maintainer chose the strongest of three options, and the risk named at the
time was that some positions may be unreachable without changing layout in ways
the grid or the line limit forbid. **Establish that before four tasks are spent
on it.**

- [ ] **Step 1: Take the three hardest positions and hand-build the target output**

- `alphabet /* c */ ab { '_' }` — the currently-unstable one.
- A rule inside an aligned grid with a comment between `->` and `write`, where
  neighbouring rules set the column widths.
- A line comment in a header (`alphabet // c`), which forces a break mid-declaration.

For each: write the output the rule demands, feed it back through `tmt fmt`, and
record whether it is a fixed point **today**. A target that is already a fixed
point is reachable; one the current printer rewrites needs the reason understood
before Task 2 starts.

- [ ] **Step 2: Answer the grid question specifically**

A comment inside a rule currently takes that rule OFF the shared grid. Under the
new rule the comment stays inline — does the rule stay off the grid, or does it
rejoin and widen its neighbours' columns? Both are defensible; the plan needs one
answer, and it must be the same for block and line flavours. Measure what today's
printer does with an off-grid rule and say which choice preserves more.

- [ ] **Step 3: Report reachable / unreachable / needs-a-ruling**

Write to the report file. If any position is genuinely unreachable, say so with
the measurement; the plan's scope shrinks rather than the rule bending silently.

---

## Task 2: the header slot — `alphabet`, and the unstable case first

**Files:** `crates/turing-machine/src/fmt/{print,trivia}.rs`, `tests/comment_positions.rs`

`alphabet` is both the worst destination and the only unstable one, so it is the
cheapest place to prove the approach.

- [ ] **Step 1: Un-ignore the `alphabet/kw-name` and `alphabet/name-brace` cases**

Both flavours. Run: `cargo test -p mtc-turing-machine --test comment_positions`
Expected: FAIL, with the neighbour comparison naming the move.

- [ ] **Step 2: Emit the header's comments at position**

The header run is already computed — `trivia::pre_brace_comments` collects exactly
the comments between a declaration's first token and its opener. Today the
printer discards their position and re-emits them inside the body. Emit them
where they sit instead.

- [ ] **Step 3: Run the three gates**

The positions test; token preservation; idempotency. Then
`cargo test -p mtc-turing-machine --lib fmt::` — **expect `pins` failures**, and
this is the moment the plan's central constraint applies: change each failing
literal individually, and for each one state in the commit message or a comment
which two significant tokens the comment now sits between. Do not re-capture.

- [ ] **Step 4: Commit**

```bash
git commit -m "fix(turing-machine): an alphabet header's comment stays in its header"
```

---

## Task 3: the header slot — the remaining nine families

Same shape as Task 2, for `namespace`, `machine`, `state`, `routine`, `graph`,
`graft`, `bind`, `tape`, and the `entry`/`export` modifiers. `use` is already
correct and its cases stay un-ignored as a regression guard.

Do them in one task, not nine: the mechanism is shared, and splitting it would
mean nine reviews of the same change. Un-ignore all of them at once, and report
per family whether it needed anything beyond the shared fix — a family that did
is a finding worth recording.

---

## Task 4: inside a list entry

The five list surfaces (`use` paths, alphabet elements, signature parameters,
binding arguments, map pairs) currently push an interior comment to the entry's
end, ahead of the separator. This is the one group that is *consistent* today —
consistently wrong under the new rule.

Because it is consistent, a single change should fix all five. If it does not,
that asymmetry is the finding.

---

## Task 5: the action slot, and the grid

The `-> /* c */ write [...]` case measured during the audit: the comment migrates
into the following vector. Fix per Task 1's grid ruling.

---

## Task 6: documentation

- `docs/tmt/fmt.md`: the relocation roster this plan removes is replaced by the
  rule. Keep the roster's shape as a *historical* note only if it aids a reader
  migrating a file; otherwise delete it — a page describing behaviour the tool no
  longer has is worse than a shorter page.
- `CLAUDE.md`: the idempotency exception. If Task 2 removed the only
  non-idempotent shape, then `.tmc` fmt becomes unconditionally idempotent, and
  the "one exception per source language" sentence becomes `.pmc`-only. **Verify
  that by running the audit, not by assuming Task 2 achieved it.**
- Every sentence written from tool output, not from memory. This repository's
  formatter documentation has been wrong four times in the immediately preceding
  plan, each time because a claim was written from recall.

---

## Exit criteria

- `crates/turing-machine/tests/comment_positions.rs` has no `#[ignore]` left for
  any position this plan declared in scope, and the four already-correct groups
  still pass.
- Token preservation and idempotency hold over the corpus, the adversarial set
  and the 122 audit positions.
- Every changed `pins` literal is individually justified; no bulk re-capture
  appears in any commit.
- `git diff --stat <plan-base> -- crates/core crates/post-machine` prints nothing.
- `docs/tmt/fmt.md` and `CLAUDE.md` describe the never-move rule, and any claim
  about idempotency was measured after the change rather than predicted.

## Out of scope

- **`.pmc`.** It has the same class of behaviour and its own non-idempotent shape
  (a comment between two stacked labels). If this plan's approach works, PM is a
  follow-up plan, not a widening of this one.
- **The five-surface multi-line rendering gap** (`paren_list`/with-map break to
  multi-line on any interior comment). Documented, deliberately unasserted, and
  unrelated to placement.
