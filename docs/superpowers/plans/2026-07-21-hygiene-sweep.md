# Hygiene Sweep Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Clear the internal-hygiene backlog before the manifest/build rounds and the release: #22, #52, #40, #39, #21's remaining body, #20, the #38 retrofit, plus test-isolation fixes for two known parallel-run flakes.

**Architecture:** Independent internal improvements across all three crates. No user-visible behavior changes except documented diagnostics/docs additions; every refactor is behavior-frozen by the existing suites.

**Tech Stack:** Rust; no new dependencies.

## Global Constraints

- PM-1 goldens untouched (`git status --short crates/post-machine/tests/golden/` empty); goldens never regenerated. `crates/core` is in scope this round.
- No version-space constant moves: `TMC_LANG_VERSION` "0.1", `TM1_TMA_DIALECT_VERSION` "0.3", `PMC_LANG_VERSION`, `PM1_PMA_DIALECT_VERSION`, `IR_VERSION`, `TM_IR_VERSION`, container formats.
- Behavior-freeze discipline for refactors (T1, T7, helpers in T6): identical findings/diagnostics/outputs, proven by the existing suites passing unchanged — a refactor task may not edit an existing test's expectations without reporting why.
- Comments cite durable docs pages only; no `spec §N` / tracker refs / `docs/superpowers/` paths in code or published docs; published docs forge-agnostic; NO AI attribution; conventional commits with scope; branch `hygiene-sweep`; branch check before every commit.
- Per task end: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check` green.
- Verify cited claims against reality; trust reality over the plan and record contradictions.

---

### Task 1: Resolve-only assemble entry (#22)

**Files:** `crates/core/src/asm/` (new resolve-only entry alongside `assemble`), core's `asm::lint` entry (stop re-running parse+lower via `assemble`), `crates/post-machine/src/lsp/pma/` and `crates/turing-machine/src/lsp/tma/` `did_update` paths (stop parsing the CST a third time where they do).

**Requirements:** Core's lint entry currently builds its rule context via `parse_asm_cst` + `lower`, then calls `assemble` as the fatal gate — which re-runs both. Add a core entry that consumes the already-produced CST+lowering and performs only the remaining assembly/validation work; rewire the lint entry and both `.pma`/`.tma` service paths to single-parse. Contract: byte-identical findings, fatals, and objects (the whole point is doing the same work once). Report the before/after redundancy (the issue measured ~25ms debug on a 10k-line file — re-measure with the same method).

- [ ] Failing-shape proof: a test or assertion demonstrating single-lowering (e.g. entry API used by lint; existing suites green unchanged)
- [ ] Implement; re-measure; full gates
- [ ] Commit: `polish(core): resolve-only assemble entry — lint and the services lower once`

### Task 2: Error-code inventory + guards (#52, + the pmt cli-page guard debt)

**Files:** every error enum with a `code() -> &'static str` match (`crates/turing-machine/src/compiler.rs`, `crates/post-machine/src/compiler.rs`, `crates/core/src/asm/mod.rs`, `crates/core/src/vm/machine.rs` — audit for others, e.g. linker/format errors, and cover whichever render bracketed codes); `docs/pmt/cli.md` + `docs/tmt/cli.md` (+ `docs/core.md` if shared namespaces land there — implementer decides placement per the issue and records why); new drift-guard tests; **plus** a `cli_docs`-style byte-for-byte `pmt --help` guard for `docs/pmt/cli.md`'s verbatim quotes (the tmt page has one; the pmt page never did).

**Requirements:** Each `code()` match becomes a lookup over a `const` table the method AND a completeness guard read (a bare match cannot be enumerated); docs gain terse per-namespace inventory tables (code + one-line trigger, `internal-error` marked as the report-a-bug bucket); every documented trigger verified against real output; rendered codes byte-unchanged (pure refactor + docs).

- [ ] Registry-ify with guards (one commit per crate acceptable); docs tables verified against the binary; pmt help guard
- [ ] Commit(s): `polish(<crate>): error codes as const registries with completeness guards`, `docs(pmt,tmt): error-code inventory tables`, `test(post-machine): pmt cli page help-quote drift guard`

### Task 3: Bidirectional directive drift guard (#40)

**Files:** core (a caps-aware directives inventory — core owns `.section`/`.row`/`.targets`/`.rept`/`.frame`-family recognition in `asm/cst.rs`; expose the inventory per `AsmCaps` as a small const/fn), the grammar drift-guard suites (turing + pm) gaining the assembler→grammar direction.

**Requirements:** A directive added to the assembler with no grammar entry must fail a test. The inventory is core-owned truth (no second hand-list); guards compare set-equality per dialect's enabled caps. Mutation-test it once (add a fake directive locally, watch the guard bite, revert — evidence in the report).

- [ ] Inventory + guards + mutation evidence; commit: `test(core,editors): directive drift guard is bidirectional`

### Task 4: `.tma` service span-convention unification (#39)

**Files:** `crates/turing-machine/src/lsp/tma/` (completions treat operand spans as inclusive; navigation as half-open — same spans).

**Requirements:** Unify on HALF-OPEN (core `Span` is end-exclusive — proven during the fold-fmt work). Adjust whichever path holds the inclusive reading; pin the boundary with tests: cursor exactly at operand end, cursor at operand start, adjacent operands. Behavior at non-boundary positions unchanged (existing suites).

- [ ] Boundary tests (failing where the conventions disagree); unify; commit: `fix(turing-machine): tma service reads operand spans half-open everywhere`

### Task 5: Core LSP hardening (#21, core half)

**Files:** `crates/core/src/lsp/` (transport, server, docstore, semantic-token packer) + tests.

**Requirements (from the issue, verify each against current code):** (1) transport `Content-Length` cap (64 MiB → dedicated error) + header variant tests (missing colon, non-numeric); (2) post-`shutdown` requests error `InvalidRequest` (except `exit`); (3) deterministic position-mapping unit pins (empty text, trailing newline, `line == 0` clamp); (4) `SemToken` packer saturating/skip fallback for contract-violating multi-line spans in release; (5) DocStore polish (derives, re-open-overwrites test, empty `uris()` test); (6) full-sync `contentChanges` multi-element test (last element wins).

- [ ] Implement + tests; commit: `polish(core): lsp hardening — transport cap, shutdown guard, packer saturation, docstore pins`

### Task 6: PM-side helpers + named test add-ons (#21, pm half)

**Files:** `crates/post-machine/src/cli/` (`render_fatal` helper centralizing the 4× repeated `{file}:{line}:{col}: error: {kind} [{code}]` format), `crates/post-machine/src/` (`lower_and_merge` helper deduplicating `analyze()`/`analyze_staged()`'s flatten→lower→warnings-merge tail — ordering contract stated once), LSP config cache bound (mtime cache eviction), plus the issue's named test add-ons (repeated-undeclared-name fixture; `span_contains` end-exclusive boundary; blank check-arm `Label` navigation branch; non-`std` import-binding semantic-token fixture; completion prefix at token end; code-action span abutting; lint-state success→failure transition).

- [ ] Helpers (behavior-frozen) + tests; commit: `polish(post-machine): render_fatal and lower_and_merge helpers, lsp cache bound, review-named test pins`

### Task 7: pmc LSP CST-walk unification (#20)

**Files:** `crates/post-machine/src/lsp/` (`navigate.rs`, `complete.rs`, `tokens.rs`) + a shared walk module.

**Requirements:** Collapse the three enclosing-function walkers, the two label-definition scans, and the two label-reference extractors into shared helpers. Behavior-frozen: every existing navigate/complete/tokens test passes unchanged; no public surface moves.

- [ ] Refactor; suites unchanged-green; commit: `polish(post-machine): one CST walk shared by navigate, complete, tokens`

### Task 8: The #38 retrofit — quickfixes for pre-existing `.tmc` rules

**Files:** `crates/turing-machine/src/lint/rules/` (the fourteen pre-existing rules), `docs/tmt/lint.md` fix-availability notes.

**Requirements:** Audit each rule for a SAFE, purely textual fix. Expected shape (verify, don't assume): deletion-fixes plausible for the older unused-* family (graph/binding/graft-instance) and possibly dead-rule (remove the covered rule, MaybeIncorrect); everything judgment-dependent (`line-too-long`, `state-may-trap`, `deprecated-call`, thresholds, naming rules) ships `fix: None` with the reason in the rule doc. Every added fix gets the apply → re-lint-clean → still-compiles test; `docs/tmt/lint.md` rows updated from real behavior. #38 closes at merge.

- [ ] Audit table (rule → fix/None+reason) in the report; implement fixes + apply-tests; docs; commit: `feat(turing-machine): quickfix retrofit — safe fixes for the pre-existing lint rules`

### Task 9: Test isolation + micro-items

**Files:** `crates/turing-machine/tests/lint_programs.rs` (the `a_bad_tmt_json_is_a_per_file_error` race: shared `CARGO_TARGET_TMPDIR` + nearest-ancestor `tmt.json` discovery — isolate each test's fixture tree so ancestor discovery cannot escape it), `crates/turing-machine/tests/mode_equivalence.rs` (the `tape set` unwrap race — diagnose: distinct temp paths or serialize the contended resource), `crates/core/src/linker/mod.rs` (micro: the `MonoRawFrame`/`MonoHoleyMatchBranch` message OPENS with "the mono call mechanism cannot lower…" even when the user invoked hybrid — reword the opening to name what happened mechanism-neutrally while keeping the frames advice; update test assertions), plus a PROBE (not necessarily a fix): the flagged "narrowest alphabet card ≥ 127 re-triggers the shared 7-bit match-ceiling internal error" — reproduce the exact shape; if the fix is small and local, take it; if architectural, write the issue-ready repro into the report for filing.

- [ ] Race fixes proven by repeated parallel runs (`cargo test --workspace` ×3 green); micro-reword; 127 probe verdict; commit(s): `test(turing-machine): isolate tmt.json and tape-set fixtures from parallel-run races`, `polish(core): stamping refusals name the mechanism neutrally`

---

## Final gates (whole branch)

- `cargo test --workspace` ×3 consecutive (the race fixes must hold under repetition) / clippy `-D warnings` / `fmt --check`
- PM goldens empty; version-space constants untouched
- Issues closed on merge: #22, #52, #40, #39, #20, #38; #21 closes (body shipped; the cherry-pick landed earlier)
