# C2 plan 9 — the `.tmc` compiler front on the green tree

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Route both `.tmc` compiler entry points — `analyze` and `analyze_staged` — through the green tree, so `parse_cst` survives only as fmt's input and as half the differential oracle.

**Architecture:** Plan 8 built typed views, a retokenization bridge and `extract_program`, and held them struct-equal to `lower_cst(parse_cst(...))` by two oracles. Nothing consumed them. This plan makes the compiler front the first consumer, exactly as PM's plan 4 did. The green tree needs a comment-bearing token stream, so `analyze` moves from `lex` to `lex_with(WithComments)` — and that, not the parse swap, is where this plan's risk lives.

**Tech Stack:** Rust, `mtc-turing-machine`. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-17-c2-green-tree-syntax-design.md`

## Global Constraints

- `crates/core` and `crates/post-machine` must show a **zero-line diff** for the whole plan. This is a standing neutrality gate, not a per-task check.
- The parser's **acceptance must not change**. Error kinds, error spans, and what parses must be identical before and after.
- Code comments cite durable `docs/` pages by page + parenthetical topic keyword, and the keyword **must resolve to a literal heading** on that page — grep and confirm. Never cite `docs/superpowers/` specs or plans; never `spec §N`.
- Published content (code comments included) is forge-agnostic: no issue/PR numbers, no hosting URLs.
- Never append `Claude-Session:` or any Claude/Claude Code attribution to a commit message or any file.
- A doc comment describing tree **shape** pastes a `debug_dump`; it never describes the nesting in prose. A doc comment giving a **reason** names the measurement that established it — a reason is measured by removing the thing and looking at what breaks. Eleven false doc sentences shipped across this migration; every one was written from inference.
- Any fixture beyond a task's own must be run through `./target/release/tmt fmt --check <file>` before it is trusted. A fixture that parses is not yet a fixture that discriminates: for each assertion, name a plausible wrong implementation that would still pass it.
- Probes go in the session scratchpad, never inside the repo. Never `rm -rf` a directory inside the repo.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` stay clean.
- **`tests/syntax_parity.rs` and `tests/tmc_property.rs` are now regression gates on every commit**, not plan-8 artifacts. Plan 8 left `extract_program` with no production consumer; this plan gives it one, so those two oracles become the net under live code.

---

## What was verified before this plan was written

Read this before Task 1; it is the difference between the scope this plan has and the scope the arc's notes predicted.

- **`expand.rs` rides free.** It contains **zero** `cst::` references — it works on `Program` and the AST types. The arc's memory predicted `expand.rs` as a major part of this plan. It is not part of it at all.
- **There are exactly two production `parse_cst` callers in the crate**, plus fmt: `compiler.rs::analyze_staged` (line ~1282) and `fmt.rs` (line ~178). `fmt.rs` belongs to plan 11 and **must not be touched here**. `analyze` reaches the CST through `parser::parse`, which is `lower_cst ∘ parse_cst`.
- **`stdlib::analysis()` rides free**: it calls `analyze_staged(SOURCE)`, so migrating `analyze_staged` migrates the stdlib roster and doc map with it. No separate task, unlike PM's plan 4.
- **`significant` already exists** at `crates/turing-machine/src/lsp/mod.rs:783` as `pub(crate) fn significant(tokens: &[Token]) -> Vec<Token>`, filtering `TokenKind::Comment`. It does not need writing — it needs moving. PM's twin lives at `crates/post-machine/src/parser.rs:342` as `significant_tokens`.
- **The compiled-stdlib gate exists**: `crates/turing-machine/tests/stdlib_golden.rs` runs the stdlib at `-O0` and `-O1` across all three `--call-mech` lowerings. It is the regression check for this plan.

## The hazard this plan exists to survive

`lint/mod.rs` passes `&analysis.tokens` into `LintContext`, and three quickfix helpers scan that stream **by adjacency**, not by predicate search:

- `braced_world_decl_span` and `reuse_statement_span` in `crates/turing-machine/src/lint/rules/spans.rs`
- `decl_span` in `crates/turing-machine/src/lint/rules/unused_alphabet.rs`

Each finds the declared name by span, then does `start_ix = name_ix.checked_sub(1)` and requires `tokens[start_ix]` to be the keyword, returning `None` otherwise. Each then calls a backward walk over a **contiguous** doc/attention run.

So once `analysis.tokens` carries comments, two things happen on any commented declaration, and **both are silent**:

1. A comment between the keyword and the name makes `is_kw` fail → the quickfix **disappears**, while the diagnostic still fires.
2. A comment between a doc run and its declaration stops `back_over_doc_run` early → the fix span **starts in the wrong place**, producing a corrupt edit.

A test that asserts a diagnostic fired, or counts diagnostics, catches neither. **Assert the applied text of the fix.** This is the same defect class as the sibling plan's `label_break`, which survived eight green mutation-armed reviews.

---

### Task 1: pin the quickfix edits by applied text, before anything moves

**Files:**
- Test: `crates/turing-machine/tests/lint_quickfix_comments.rs` (create)

**Interfaces:**
- Consumes: nothing new.
- Produces: a fixture that passes today and is expected to FAIL in Task 2 before its fix lands. That failure is the point of this task.

- [ ] **Step 1: Find how an existing test applies a fix**

Read how the crate's lint tests already turn a `Fix` into edited text — grep `tests/` for `Fix` and for `lint(` and follow one rule's existing quickfix test. Reuse that helper rather than inventing one. If no helper applies a fix to source, write one in this file: take the fix's span, splice its replacement into the source, and return the result.

Say in your report which existing test you followed.

- [ ] **Step 2: Write the fixtures**

Three declaration shapes, each in two variants — clean, and with a comment in the position that breaks adjacency. Run every one through `./target/release/tmt fmt --check` first.

The three shapes and the rule each drives:

| helper | rule | declaration |
|---|---|---|
| `decl_span` | `unused-alphabet` | `alphabet` |
| `braced_world_decl_span` | `unused-routine` | `routine` |
| `reuse_statement_span` | `unused-binding` | `bind` |

For each, assert the **applied text**, not that a fix exists:

**The `unused-alphabet` pair, verified through the real `tmt` binary before this plan shipped.** Both variants parse, and each raises EXACTLY ONE diagnostic — which is what makes "apply the fix" unambiguous:

```
alphabet ab { '_' }

alphabet cd { '_' }

machine {
  tape main: cd;
  entry state s { [*] -> move [>] stop; }
}
```

and the commented variant, identical but for the first line:

```
alphabet /* c */ ab { '_' }
```

Three things about that fixture are not guessable and were each wrong in a first draft:

- **A `machine` needs an entry.** Without `entry state s { … }` the source fails `entry-count` before any lint runs, so `unused-alphabet` never fires at all.
- **The rule must move the tape.** With `[*] -> stop;` the fixture ALSO raises `unused-tape` on `main`, and a two-diagnostic fixture makes "the fix" ambiguous. `move [>]` uses the tape and leaves exactly one diagnostic.
- **`cd` must stay used**, or it becomes a second `unused-alphabet`.

Verified output for both variants, one line each: `alphabet 'ab' is never used by any tape`.

**Derive each expected string by hand from what the fix should do — never capture it from a run.** That is this repo's golden discipline and the only thing that makes the assertion an oracle rather than a snapshot. Two properties it must satisfy, and state them in the test's comment: the `ab` declaration is gone, and `cd`, the machine, and the blank lines between them are byte-identical to the input.

Write the `routine` and `bind` pairs the same way, and run each through `./target/release/tmt lint` first to confirm it raises exactly one diagnostic.

- [ ] **Step 3: Run them and watch them PASS**

Run: `cargo test -p mtc-turing-machine --test lint_quickfix_comments`
Expected: PASS. Today `analysis.tokens` is comment-free, so the comment variants are already safe. A failure here means the expected string is wrong, not the code.

- [ ] **Step 4: Prove they discriminate**

Make `back_over_doc_run` return `start_ix` unchanged (never walking back over a doc run). Confirm a named test in this file fails, and quote it. Restore.

Then make `decl_span` return `None` unconditionally, confirm a named test fails, quote it, restore.

A fixture that cannot see either change is not pinning the edit.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/tests/lint_quickfix_comments.rs
git commit -m "test(turing-machine): pin .tmc lint quickfix edits by applied text"
```

---

### Task 2: `analyze` onto the green tree

**Files:**
- Modify: `crates/turing-machine/src/compiler.rs`, `crates/turing-machine/src/parser.rs`, `crates/turing-machine/src/lsp/mod.rs`, `crates/turing-machine/src/lint/mod.rs`

**Interfaces:**
- Consumes: `parse_green_from_tokens`, `syntax::extract_program`, Task 1's fixture.
- Produces: `parser::significant_tokens` (moved out of `lsp/mod.rs`, made `pub(crate)` at the parser level); `analyze` no longer calls `parser::parse`.

- [ ] **Step 1: Move `significant` to the parser and rename it**

`crates/turing-machine/src/lsp/mod.rs:783` holds `pub(crate) fn significant(tokens: &[Token]) -> Vec<Token>`. Move it to `crates/turing-machine/src/parser.rs` as `significant_tokens`, mirroring the sibling at `crates/post-machine/src/parser.rs:342`. Update `lsp/tokens.rs`'s call site.

It belongs to the parser now because the compiler front, not the language service, is about to be its main caller. Say that in its doc comment, and say what it filters — `TokenKind::Comment` only, since `.tmc` has no other trivia token kind at the lexer level.

- [ ] **Step 2: Switch `analyze`**

```rust
pub(crate) fn analyze(source: &str) -> Result<Analysis, CompileError> {
    let tokens = lex_with(source, LexMode::WithComments)?;
    let green = parse_green_from_tokens(source, &tokens)?;
    let program = crate::syntax::extract_program(&SyntaxNode::new_root(green), source);
    let (resolved, diagnostics) = resolve_program(&program)?;
    Ok(Analysis {
        resolved,
        diagnostics,
        program,
        tokens,
    })
}
```

Note what changed and what did not: the lex mode, the parse, and nothing else. `Analysis.tokens` now carries comments — that is the hazard, and Step 3 is where it shows.

- [ ] **Step 3: Run Task 1's fixture and WATCH IT FAIL**

Run: `cargo test -p mtc-turing-machine --test lint_quickfix_comments`

Expected: the comment variants FAIL. Quote the exact failures in your report — this is the plan's central claim being demonstrated rather than asserted. If they all still pass, STOP and report it: either the fixture is not reaching the adjacency check, or `analyze` did not actually change stream, and both are worth knowing before going further.

- [ ] **Step 4: Feed lint the significant stream**

In `crates/turing-machine/src/lint/mod.rs`, `LintContext.tokens` must receive `significant_tokens(&analysis.tokens)` rather than `&analysis.tokens`. `comment_tokens` keeps its current meaning.

Say at that call site WHY the filter is there — that three quickfix helpers scan by adjacency and a comment between a keyword and its name silently voids or shifts the edit — and name the test that proves it.

- [ ] **Step 5: Run everything**

Run: `cargo test -p mtc-turing-machine`
Expected: green, Task 1's fixture included.

Then run the compiled-stdlib gate explicitly and say so in your report:

```
cargo test -p mtc-turing-machine --test stdlib_golden
```

- [ ] **Step 6: Prove acceptance did not change**

For every `.tmc` in the corpus (`tests/golden`, `src/stdlib`, `../../docs/examples`) plus a handful of deliberately broken sources, assert that the old path and the new path produce the same `Result` — the same `Ok`, or the same error kind at the same span. Write this as a test, not as a one-off probe.

The broken sources matter more than the good ones: this plan changes which function produces the error.

- [ ] **Step 7: Commit**

```bash
git add crates/turing-machine/src
git commit -m "feat(turing-machine): analyze parses .tmc through the green tree"
```

---

### Task 3: `analyze_staged` onto the green tree

**Files:**
- Modify: `crates/turing-machine/src/compiler.rs`

**Interfaces:**
- Consumes: Task 2's work.
- Produces: `analyze_staged` builds its `program` from the green tree; `TmcStagedAnalysis.cst` stays populated.

- [ ] **Step 1: Keep the CST field, and say why**

`TmcStagedAnalysis.cst` feeds the not-yet-migrated `.tmc` language service (`lsp/mod.rs` and `lsp/quickfix.rs` both import `crate::cst::`). It stays populated, so this task leaves an **interim double parse** — one `parse_cst` for the LSP, one `parse_green_from_tokens` for the program. Plan 10 removes it, exactly as the sibling's plan 5 did.

Write that at the field, with the words "interim" and the name of the plan that removes it, so it is not mistaken for a design choice.

- [ ] **Step 2: Switch the program construction**

Keep the staged shape — every early return, every field — and change only where `program` comes from. `lower_cst(&cst)` becomes extraction from the green tree. `resolve_program` and everything after it is untouched.

The fatal ordering must not change: a lex failure returns with `tokens: None`, a parse failure with `tokens: Some(...)`, `cst: None`.

- [ ] **Step 3: Run the staged tests**

Run: `cargo test -p mtc-turing-machine --lib compiler`
Expected: green, including the tests that assert `staged.tokens` is a genuine `WithComments` stream.

- [ ] **Step 4: Prove the two paths still agree**

`analyze` and `analyze_staged` now build a `Program` by the same route. Add one test asserting they produce an equal `program` for a source carrying comments, a doc run, and a namespace — the shapes where the two lex modes used to differ.

- [ ] **Step 5: Commit**

```bash
git add crates/turing-machine/src
git commit -m "feat(turing-machine): analyze_staged builds its program from the green tree"
```

---

### Task 4: collapse lint's double lex

**Files:**
- Modify: `crates/turing-machine/src/lint/mod.rs`

**Interfaces:**
- Consumes: Tasks 2-3.
- Produces: one lex per `lint()` call.

- [ ] **Step 1: Remove the second lex**

`lint()` currently lexes twice — once inside `analyze`, then again explicitly as `lex_with(source, LexMode::WithComments).unwrap_or_default()`. After Task 2, `analysis.tokens` **is** the comment-bearing stream, so `comment_tokens` can come from it directly and `tokens` from `significant_tokens`.

- [ ] **Step 2: Delete the comment that justified it**

The existing comment argues the second lex cannot fail because "the comment-free lex already succeeded". That justification dies with the change. Remove it; do not adapt it. A stale justification for code that no longer exists is worse than no comment.

- [ ] **Step 3: Prove the behaviour is identical**

Run the full lint test surface and the corpus:

```
cargo test -p mtc-turing-machine --lib lint
cargo test -p mtc-turing-machine --test lint_programs
```

Then prove the removal is real rather than cosmetic: assert somewhere that `lint()` lexes once. A counter is not available, so instead state in your report what you checked — for instance that `lex_with` appears exactly once on the `lint()` path — and how you established it.

- [ ] **Step 4: Commit**

```bash
git add crates/turing-machine/src/lint/mod.rs
git commit -m "polish(turing-machine): one lex per .tmc lint run"
```

---

### Task 5: documentation

**Files:**
- Modify: `CLAUDE.md`, `crates/turing-machine/src/syntax/mod.rs`

**Interfaces:**
- Consumes: Tasks 1-4. No code changes.

- [ ] **Step 1: Record what is true now, and only that**

`CLAUDE.md`'s `### The `.tmc` front end` paragraph currently says nothing production-side calls the green tree. After this plan, the compiler front does, and `parse_cst` survives as fmt's input plus half the differential oracle plus the LSP's interim source.

Make the smallest true edit. Keep `CLAUDE.md` at standing state — do not narrate this plan or its task count.

- [ ] **Step 2: Record the consumer in the module doc**

`syntax/mod.rs` carries the retro-wrap rulings and the oracle paragraph. Add that extraction now has a production consumer and that both oracles are therefore regression gates rather than plan artifacts.

**Verify every sentence.** For each, name the specific thing that would make it false and check that thing by running something. A tree-shape claim gets a dump or a measured range, never a bare count — and that binds a correction as hard as an original.

- [ ] **Step 3: Verify**

Run: `cargo test --workspace`

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md crates/turing-machine/src/syntax/mod.rs
git commit -m "docs: the .tmc compiler front runs the green tree"
```

---

## Exit criteria

- `analyze` and `analyze_staged` both build their `Program` from the green tree; no production `parser::parse` or `lower_cst` call remains on either path.
- `fmt.rs` still calls `parse_cst` — untouched, and plan 11's job.
- `TmcStagedAnalysis.cst` remains populated for the language service, documented as interim with the plan that removes it named.
- Lint's quickfix edits are asserted by applied text, and the comment variants were **observed to fail** between Task 2's parse swap and its filter fix.
- `lint()` lexes once.
- The compiled-stdlib gate (`stdlib_golden.rs`) passes, and parser acceptance is proven unchanged over the corpus and over deliberately broken sources.
- `tests/syntax_parity.rs` and `tests/tmc_property.rs` stay green.
- `crates/core` and `crates/post-machine` have a zero-line diff for the whole plan.
