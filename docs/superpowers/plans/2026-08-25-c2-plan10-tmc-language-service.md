# C2 plan 10 — the `.tmc` language service on typed views

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Move the two `.tmc` language-service features that still read the C1 CST onto typed views, then delete both CST fields and the interim double parse — leaving `fmt.rs` as the only production `parse_cst` caller.

**Architecture:** Plan 9 put both compiler entry points on the green tree but kept `TmcStagedAnalysis.cst` populated as scaffolding, because `lsp/mod.rs` and `lsp/quickfix.rs` still read it. This plan ports those two readers and removes the scaffolding. The sibling did exactly this between its own plans 4 and 5.

**Tech Stack:** Rust, `mtc-turing-machine`. No new dependencies.

**Spec:** `docs/superpowers/specs/2026-08-17-c2-green-tree-syntax-design.md`

## Global Constraints

- `crates/core` and `crates/post-machine` must show a **zero-line diff** for the whole plan.
- Code comments cite durable `docs/` pages by page + parenthetical topic keyword, and the keyword **must resolve to a literal heading** — grep and confirm. **No `docs/superpowers/` spec or plan is ever cited by code**, frozen or active; carry the substance in prose.
- Published content (code comments included) is forge-agnostic: no issue/PR numbers, no hosting URLs.
- Never append `Claude-Session:` or any Claude/Claude Code attribution to a commit message or any file.
- A doc comment describing tree **shape** pastes a `debug_dump`. A doc comment giving a **reason** names the measurement that established it. A claim carrying a **count** gets a measured range or a dump, never a bare number. Twelve false doc sentences shipped across this migration, three of them written inside a fix for another.
- **Write the invariant and its enforcement, never a neighbour's current reason for satisfying it.** Plan 9 shipped a *staled* sentence — true when written, false two tasks later when the neighbour moved. A false sentence is caught by checking it; a staled one passes every check on the day it is written.
- Any fixture: run it through `./target/release/tmt lint <file>` first. A fixture that parses is not yet a fixture that discriminates — for each assertion, name a plausible wrong implementation that would still pass it.
- **Mutation runs use `--no-fail-fast` and cover the crate, not one target.** `cargo test` is fail-fast across targets and hides the rest. **A claimed NEGATIVE is the one thing a reader cannot check** — re-run it and name the command.
- **Run long test suites in the FOREGROUND and wait.** Two agents in this arc stalled by backgrounding a run and waiting for a completion notification. Nothing sends one; the agent waits forever.
- Probes go in the session scratchpad. A Rust probe that must compile inside the repo is created as ONE named file and deleted by that exact name — never `rm -rf` a directory.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, and `cargo test --workspace` stay clean. `tests/syntax_parity.rs` and `tests/tmc_property.rs` are standing regression gates.

---

## What was measured before this plan was written

**The arc's notes predicted this plan would be "genuinely harder than the sibling's plan 5" because the `.tmc` LSP "really does hold the CST".** That prediction was sized on the LSP's 8,671 lines, not on how far the CST actually reaches. It reaches **two features**:

| site | what it uses the CST for |
|---|---|
| `lsp/mod.rs:743` (`document_symbols`) | walks top items → `namespace_symbol` / `reuse_symbol` / `machine_symbol`, each needing name, `span`, `name_span`, and nested items |
| `lsp/quickfix.rs:29` (`state_stub` → `enclosing_body`) | finds the innermost world block containing a position, and its closing brace |

`lsp/mod.rs:655` is the assignment that feeds `DocState.cst`. That is the whole surface. Everything else in `lsp/` — `overlay.rs` (1,828 lines), `navigate.rs`, `complete.rs`, `context.rs`, `roster.rs` — already runs off `Program`/`Resolved` and is untouched by this plan.

**The decisive measurement, run before writing.** On `"alphabet ab { '_' }\n\n? doc\nmachine {\n …\n}\n"`:

```
CST   span: start Pos { line: 4, col: 1 }  end Pos { line: 7, col: 2 }
GREEN node: start (3,1)                    end (7,2)
END MATCH: true
START MATCH: false
```

Two consequences, and they split this plan:

- **The END matches exactly**, including the 1-based char-column convention. `BodyExtent.close` is `span.end` and nothing else, so **quickfix is a mechanical port**: `TextLineIndex::line_col(node.text_range().end)` reproduces it.
- **The START differs by retro-wrap** — the green node opens at the `? doc` line, the CST at the `machine` keyword. So every symbol `span` needs the doc run skipped.

The sibling's precedent is `crates/post-machine/src/lsp/mod.rs:458`, `fn function_extent(f: &FunctionView) -> TextRange`, which takes the first child that is not a DOC_RUN or trivia and re-bases the range's start on it.

**The retro-wrap trap is wider here than on the sibling.** PM had ONE retro-wrapping kind at symbol level (FUNCTION). Plan 8 measured **four** for `.tmc` — NAMESPACE, ALPHABET, REUSE and MACHINE all carry their own doc run as a direct first child (WORLD has none; a run before the first body item retro-wraps INTO that item). A helper written for one kind and reused for three without checking each is the shape that shipped the `top_items` assert which panicked on a documented namespace.

---

### Task 1: the symbol extent helper, and `document_symbols` onto views

**Files:**
- Modify: `crates/turing-machine/src/lsp/mod.rs`
- Test: `crates/turing-machine/src/lsp/tests.rs`

**Interfaces:**
- Consumes: `crate::syntax` views (`RootView`, `TopView`, `NamespaceView`, `ReuseView`, `MachineView`, `AlphabetView`, `StateView`, `GraftView`, `BindView`), `TextLineIndex`.
- Produces: a `symbol_extent` helper and `document_symbols` reading the green tree.

- [ ] **Step 1: Write the failing tests**

Four fixtures, one per retro-wrapping kind, each with a documented declaration, asserting the symbol's span starts at the **keyword** and not at the doc run. Assert by VALUE — the line and column — not by "a span exists".

```rust
/// A documented declaration's green node opens at its doc run, so the
/// raw node extent is NOT the symbol's range: a client revealing it
/// would scroll to the comment above the declaration. Every symbol
/// level re-bases its start past the doc run and any trivia. All four
/// kinds that can carry a doc run are covered, because a helper written
/// for one and reused for three is how a wrong assumption ships.
#[test]
fn a_documented_declarations_symbol_starts_at_its_keyword() {
    // one case per kind: namespace, alphabet, reuse, machine
}
```

Also pin what `world_symbols` includes. Its current doc says tape declarations and comments are not symbols; on views that becomes "which node kinds are symbols", and an accidentally-inclusive walk adds symbols nothing counts. **Assert which kinds appear, not how many** — the same discipline the doc-run fixture in `tests/syntax_views.rs` uses.

And pin `machine_symbol`'s selection span. It is currently a synthesized `Span::point(m.line, m.col)` because a machine block has no name token. Decide its green source — the `machine` keyword IDENT's own start is the natural one — assert it by value, and say at the definition site why it is synthesized rather than read.

- [ ] **Step 2: Run them and watch them fail**

Run: `cargo test -p mtc-turing-machine --lib lsp`
Expected: FAIL — the helper does not exist.

- [ ] **Step 3: Write the implementation**

Port `symbol_extent` from the sibling's `function_extent`, then rebuild `cst_symbols`/`namespace_symbol`/`reuse_symbol`/`machine_symbol`/`world_symbols` over views. Read the sibling first.

The trivia kinds to skip are `.tmc`'s own — check `syntax::kinds` for the trivia range rather than copying the sibling's `PmcKind` names.

- [ ] **Step 4: Run them and watch them pass**

Run: `cargo test -p mtc-turing-machine --lib lsp`

- [ ] **Step 5: Prove each level bites**

For EACH of the four kinds, make `symbol_extent` return the raw node extent for that kind alone, run the crate with `--no-fail-fast`, and confirm a named test fails. Quote each. Restore between.

- [ ] **Step 6: Commit**

```bash
git add crates/turing-machine/src/lsp
git commit -m "feat(turing-machine): document symbols read the .tmc green tree"
```

---

### Task 2: `state_stub` onto views

**Files:**
- Modify: `crates/turing-machine/src/lsp/quickfix.rs`
- Test: `crates/turing-machine/src/lsp/tests.rs`

**Interfaces:**
- Consumes: Task 1's views work.
- Produces: `enclosing_body` walking the green tree.

- [ ] **Step 1: Write the failing test**

The fix inserts a stub state one level in from the world's closing brace, and the indent is read off that brace's column. So the test that matters asserts the **inserted text and its position**, not that an action exists.

Cover the case the current code's own comment calls out: a world nested in a namespace sits deeper than a top-level `machine`, and a stub indented for the wrong depth leaves the file failing `tmt fmt --check`. Include a nested world, and assert the indent.

- [ ] **Step 2: Run it and watch it fail**

- [ ] **Step 3: Write the implementation**

`BodyExtent.close` is `span.end` and nothing more, and the measurement above shows a green node's `text_range().end` reproduces it exactly through `TextLineIndex::line_col`. So this is a mechanical port: walk `RootView::items()` recursively, keep the innermost MACHINE or REUSE whose range contains the position, and take its end.

`arity` comes from `Program`, not from the tree, and does not change.

**Do NOT re-base the start here.** Task 1's extent helper exists for symbol ranges; `enclosing_body` uses only the end, and applying the helper would be a change with no reason behind it.

- [ ] **Step 4: Run it and watch it pass**

- [ ] **Step 5: Prove it bites**

Mutate `enclosing_body` to take the OUTERMOST containing block instead of the innermost, and confirm the nested-world test fails by name. Then mutate the end to the node's start and confirm the indent assertion fails. Quote both, restore.

- [ ] **Step 6: Commit**

```bash
git add crates/turing-machine/src/lsp
git commit -m "feat(turing-machine): the state-stub quickfix reads the .tmc green tree"
```

---

### Task 3: delete both CST fields and the interim double parse

**Files:**
- Modify: `crates/turing-machine/src/compiler.rs`, `crates/turing-machine/src/lsp/mod.rs`

**Interfaces:**
- Consumes: Tasks 1-2 — both readers are gone.
- Produces: `TmcStagedAnalysis` and `DocState` without a `cst` field; one parse per language-service request.

This is its own task deliberately. The exit criterion — no production `parse_cst` outside `fmt.rs` — is checkable, and folding the removal into a feature task hides whether the last reader actually went away.

- [ ] **Step 1: Prove there are no readers left**

Grep for every `.cst` read across `crates/turing-machine/src`. Report the list; it should be empty apart from the field declarations and the assignment. If anything remains, STOP and report it — a removal with a live reader is a compile error, but a removal that silently drops a feature is worse.

- [ ] **Step 2: Remove the fields, the `parse_cst` call, and the `debug_assert!`**

`analyze_staged` stops calling `parse_cst` entirely. The `.ok()` and its `debug_assert!` go with it.

**Deleting `parse_cst` does NOT by itself give one parse per request** — task 1
found this and it would otherwise be discovered here. `document_symbols` now
reparses the green tree per request, because nothing retains it. Getting to one
parse means retaining the tree: add `green: Option<Rc<GreenNode>>` to
`TmcStagedAnalysis` and carry it onto `DocState`, the way the sibling does at
`crates/post-machine/src/compiler.rs:483`. Read that first.

So this task has two halves — remove the CST, and retain the green tree — and the
exit criterion is about the second. Measure the parse count before and after and
say how you measured it.

- [ ] **Step 3: Delete the stale doc comment carried from plan 9**

`DocState.cst`'s doc carried the same stale claim `TmcStagedAnalysis.cst` had — "`None` when lexing or parsing failed" — from before `program` moved onto the green tree. The field goes away, so the sentence goes with it; confirm no other comment in `lsp/` still describes a CST-backed tier.

- [ ] **Step 4: Verify the double parse is gone**

State how you established it — for instance that `parse_cst` no longer appears in `compiler.rs` at all, and that `fmt.rs` is its only remaining production caller. A count is not enough on its own; name the command.

- [ ] **Step 5: Run everything**

`cargo test --workspace --no-fail-fast`, in the FOREGROUND.

- [ ] **Step 6: Commit**

```bash
git add crates/turing-machine/src
git commit -m "polish(turing-machine): one parse per .tmc language-service request"
```

---

### Task 4: documentation

**Files:**
- Modify: `CLAUDE.md`, `crates/turing-machine/src/syntax/mod.rs`

**Interfaces:**
- Consumes: Tasks 1-3. No code changes.

- [ ] **Step 1: Record what is true now**

`CLAUDE.md`'s `.tmc` paragraph currently lists `parse_cst`'s three surviving roles. After this plan there are two: `fmt`'s input, and half the differential oracle. Make the smallest true edit; keep the file at standing state and do not narrate this plan.

- [ ] **Step 2: Check every sentence**

For each, name the specific thing that would make it false and check THAT by running something. Report as `sentence → what you ran → verdict`.

- [ ] **Step 3: Verify**

Run: `cargo test --workspace` in the foreground.

- [ ] **Step 4: Commit**

```bash
git add CLAUDE.md crates/turing-machine/src/syntax/mod.rs
git commit -m "docs: the .tmc language service runs the green tree"
```

---

## Exit criteria

- `document_symbols` and the state-stub quickfix both read the green tree; neither imports `crate::cst::`.
- Every symbol span starts at its declaration's keyword, not at a bound doc run, proven by a fixture at each of the four retro-wrapping kinds.
- `TmcStagedAnalysis` and `DocState` have no `cst` field; `analyze_staged` parses once.
- `fmt.rs` is the only production `parse_cst` caller left, and it is untouched by this plan.
- The state-stub fix's inserted text and indent are asserted by value, including a world nested in a namespace.
- `tests/syntax_parity.rs` and `tests/tmc_property.rs` stay green.
- `crates/core` and `crates/post-machine` have a zero-line diff for the whole plan.
