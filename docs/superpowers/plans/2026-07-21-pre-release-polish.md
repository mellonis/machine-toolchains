# Pre-Release Polish Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Five small first-impression fixes before the arc release: grammar keyword re-scoping for JetBrains (#51 prong 1), the tape-new panic (#41), the MonoRawFrame advice (#42), `dis`'s missing foreign-arch refusal (#50), and case-insensitive LSP extension routing (#36).

**Architecture:** Independent point fixes. Three touch `crates/core` (formats error, linker message, LSP routing) — the previous round's core-zero-diff constraint does NOT apply here; PM-1 golden byte-identity still does. No version space moves.

**Tech Stack:** Rust; TextMate grammar JSON; no new dependencies.

## Global Constraints

- PM-1 byte-identity: `git status --short crates/post-machine/tests/golden/` stays empty; goldens never regenerated.
- No version space moves: `TMC_LANG_VERSION` "0.1", `TM1_TMA_DIALECT_VERSION` "0.3", `PMC_LANG_VERSION`/`PM1_PMA_DIALECT_VERSION`, `IR_VERSION`/`TM_IR_VERSION`, container formats — all unchanged. (Diagnostic-text and error-typing changes do not move acceptance contracts.)
- New/changed code comments cite durable docs pages only; never `spec §N`, issue/PR numbers, or `docs/superpowers/` paths. Published docs forge-agnostic. NO Claude/AI attribution.
- Conventional commits with scope. Branch: `pre-release-polish`. Branch check (`git rev-parse --abbrev-ref HEAD`) before every commit.
- Per-task finish: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check` all green.
- Verify cited file:line hints before editing; trust reality over the plan and record contradictions.

---

### Task 1: Grammar keyword re-scope (#51 prong 1)

**Files:**
- Modify: `editors/grammars/tmc.tmLanguage.json`, `editors/grammars/tma.tmLanguage.json` (verify exact filenames in `editors/grammars/`)
- Audit (fix the same way if exposed): the PM pair's `.pmc`/`.pma` grammar files in the same directory
- Test: the grammar drift-guard suites (`editor_grammar` and PM equivalents — find them; they must stay green)

**Requirements:**
1. In the `.tmc` grammar, the primary scope for declaration and control keywords becomes `keyword.control.tmc`: `machine`, `state`, `tape`, `alphabet`, `graph`, `routine`, `graft`, `bind`, `entry`, `export`, `use`, and the rule-tail keywords `write`, `move`, `goto`, `call`, `then`, `return`, `stop`, `halt`, `debugger` (verify the full keyword inventory against the lexer's reserved set — the drift guards know it; do not invent or drop members). Finer scopes that IntelliJ's fixed TextMate mapping does not recognize (`storage.type.*`, `keyword.operator.move.*`, `keyword.other.*` for keywords) are replaced, not doubled — one `name` per pattern.
2. Same treatment for the `.tma` grammar's directive/mnemonic keyword tier where it uses out-of-table scopes (audit first — its single-scope substitution rule was found sound in an earlier round; only re-scope what renders plain in IntelliJ's table: comment/string/number/keyword are the safe survivors).
3. Audit the `.pmc`/`.pma` grammars for the same exposure; apply the identical policy if present (their effect ships with the next PM plugin build — still correct to fix now).
4. Non-keyword scopes (strings, comments, numbers, wildcards, punctuation, entity names) stay as they are — this task moves ONLY the keyword tier.
5. All drift guards green; `cargo test --workspace` green.

**Steps:**
- [ ] Inventory current keyword scopes per grammar (list them in your report); confirm which fall outside IntelliJ's recognized prefixes
- [ ] Re-scope; run drift guards + full suite
- [ ] Commit: `fix(editors): grammar keyword tier scopes as keyword.control — IntelliJ TextMate mapping compatibility`

### Task 2: Typed error for oversize tape alphabets (#41)

**Files:**
- Modify: `crates/core/src/formats/tapeblock.rs` (the `u8` conversion that panics, ~:221 — re-locate) and/or the earliest layer where the width is known; the `tmt tape` CLI path (`crates/turing-machine/src/cli/`) so the error renders as a normal typed diagnostic
- Test: core formats unit tests + a turing-side CLI-level test

**Requirements:**
1. Reproduce first: a `.tma` routine declared `alpha=(300)` assembles and links; `tmt tape new` on the result panics `alphabet fits u8: TryFromIntError(())`. Capture the exact current behavior in your report.
2. Replace the panic with a typed error following core formats' existing error conventions (find the formats error enum; add a variant carrying the offending width and the container's maximum). The message names both numbers, e.g. `tape alphabet has 300 symbols; the MT container carries at most 256` (exact final wording follows the codebase's diagnostic style — match neighboring messages).
3. `tmt tape new` (and `pmt tape new` if the same path is reachable there — probe it) surfaces the error as a normal CLI error with the standard error exit code, no panic, no backtrace.
4. Consider whether the error should fire even earlier (at link or assemble time). Do NOT add new early gates in this task — that is a design change; note the observation in your report instead. This task's contract: no reachable panic.

**Steps:**
- [ ] Failing test: `tape new` path on a >256-alphabet image returns the typed error (assert code/message shape), never panics
- [ ] Implement; full gates
- [ ] Commit: `fix(core): oversize tape alphabet is a typed error, not a panic`

### Task 3: MonoRawFrame advice (#42)

**Files:**
- Modify: the `MonoRawFrame` error's message/Display in `crates/core/src/linker/` (find the exact site)
- Test: existing linker tests asserting the message; update assertions

**Requirements:**
1. Current message advises `--call-mech=frames` or hybrid; in the common failure configuration (all bound sites specializable, so hybrid delegates wholesale to mono) hybrid refuses identically. Verify that behavior once with a probe before rewording (the issue documents it; confirm it still holds).
2. New advice: recommend `frames` unconditionally. Mention hybrid ONLY if you can state, in the message or in one trailing clause, the condition under which it actually helps (a non-specializable bound site forcing the frames path) — if that reads too long for a diagnostic, drop hybrid from the advice entirely and let the docs carry the nuance.
3. Grep `docs/` for any page quoting the old message verbatim; update in the same commit (drift guards/doc quotes must agree).

**Steps:**
- [ ] Probe + failing message assertion; implement; full gates
- [ ] Commit: `fix(core): MonoRawFrame advice recommends the mechanism that works`

### Task 4: `dis` refuses foreign architectures (#50)

**Files:**
- Modify: `crates/post-machine/src/cli/inspect.rs` (the `dis` path)
- Probe + fix if same gap: `crates/turing-machine/src/cli/inspect.rs`-equivalent (`tmt dis` on a PM-1 image)
- Test: both crates' CLI test suites

**Requirements:**
1. Reproduce: `pmt run` on a TM-1 `.tmx` refuses `unknown architecture 0x02`; `pmt dis` on the same file prints well-formed PM-1 nonsense. `pmt dis` must perform the same arch check the run path does, with a message consistent with run's (reuse the same error/rendering where the code allows).
2. Probe `tmt dis` against a PM-1 `.pmx`: if it has the same gap, fix identically; if it already refuses, pin that with a test and say so.
3. Containers are identified by `sniff()` on the magic, never extensions — the check reads the arch byte from the decoded container, mirroring run's gate. Exit code matches the CLIs' standard error exit.

**Steps:**
- [ ] Failing tests (pm side; tm side per probe); implement; full gates
- [ ] Commit: `fix(post-machine): dis refuses a foreign architecture like run does` (+ `fix(turing-machine): …` if the tm side needs it — separate commit per crate)

### Task 5: Case-insensitive LSP extension routing (#36)

**Files:**
- Modify: `crates/core/src/lsp/` — `bind_service`'s extension matching (find the compare)
- Test: core LSP tests (the fake-service suite — zero PM/TM knowledge in core tests)

**Requirements:**
1. Extension matching becomes case-insensitive (`X.TMA` routes to the `.tma`-registered service; same for `.PMA`/`.PMC`/`.TMC`). `languageId` precedence and the unmatched-document fallback behavior stay exactly as they are.
2. Test in core's fake-service style: a document whose URI carries an uppercase extension binds to the right fake service; a genuinely unknown extension still falls back as today.

**Steps:**
- [ ] Failing test; implement (case-insensitive compare at the match site — do not lowercase stored registrations if that changes any public surface; compare-time folding suffices); full gates
- [ ] Commit: `fix(core): LSP extension routing is case-insensitive`

---

## Final gates (whole branch)

- `cargo test --workspace` / clippy `-D warnings` / `fmt --check`
- `git status --short crates/post-machine/tests/golden/` empty
- Version-space constants untouched (grep the five)
- Issues closed on merge: #41, #42, #50, #36; #51 gets a prong-1-shipped comment and STAYS OPEN carrying prong 2.
