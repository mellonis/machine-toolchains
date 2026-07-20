# TM-1 arc — phase 7: TM tooling

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `.tmc` and `.tma` reach full tooling parity with the PM family — lint and fmt for both languages, the two `LanguageService`s under one `tmt lsp`, `tmt completions`, `tmt.json`, and the TM editor-plugin pair — with the phase exit being everything-automated-green plus buildable plugin artifacts and written sideload checklists (live verification is the maintainer's, per the pmt precedent).

**Architecture:** everything mirrors shipped pmt precedent (the facts sheet .superpowers/sdd/phase7-facts.md is the implementers' detailed reference): the core LSP framework and `format_asm_with` are reused verbatim; `tmt lsp`/`tmt fmt`(.tma) are wiring; the TM crate's lint, `.tmc` fmt, both services, completions, `tmt.json`, and the plugins are greenfield mirrors. The one genuinely new substrate is the TM **staged analysis** seam (pmc's `analyze_staged`/`StagedAnalysis` twin) that the `.tmc` service depends on — it leads the phase. Single branch, eight tasks (no split: all templated, no VM/format changes).

**Tech stack:** Rust 2024 (no new deps in the crates); the plugin pair uses the existing `editors/` node/gradle toolchains only.

**Plan-level adjudications (recorded):**
1. **The `.tma` lint seam**: core's `asm::lint::lint` stays CLOSED (the pma precedent added zero rules and calls it directly). The TM additions run turing-side over the same asm CST and MERGE diagnostics with core's output. A shared extension seam in core is warranted only when a third dialect appears — recorded trigger, not built now.
2. **`unused-label`**: core's existing rule is reused as-is; the spec's listing of it among the TM additions is a redundancy, not a second rule (record in the lint module doc).
3. **Phase exit**: plugin artifacts BUILD and the manual sideload checklists are WRITTEN; live shell verification is the maintainer's post-merge step (pmt precedent — "both shells user-verified live" came after).
4. **No version space moves** except the birth of `MIN_TESTED_TMT` (plugin floor, set to the current crate version) and the TM plugin versions at 0.1.0.

## Global Constraints

1. **Thin renderer** everywhere; all lint/fmt/LSP surfaces return structured values; only `cli/` prints.
2. **The lossless-CST contract holds**: fmt is canonical, idempotent, whitespace-only, trivia-preserving; the fmt battery proves idempotence AND no-token-loss on every fixture (the pmc discipline).
3. **Lint allow-namespace**: one shared namespace across `.tmc`/`.tma` rules (the pmt shape); `tmt.json`'s `lint.allow` uses UNION semantics with IDE settings — never a cascade; nearest-ancestor discovery; the schema mirrors `pmt.json`'s and is documented in docs/lint.md only if that page already covers pmt.json generically — else prose in the module (docs pages are phase 8).
4. **Staged analysis** (`analyze_staged` → `TmcStagedAnalysis`): partial results at every break point (lex-fail → tokens only; parse-fail → tokens+CST-attempt; resolve-fail → +program; success → +resolved); NEVER panics on any input (property test); `compile()`'s behavior byte-unchanged (the staged seam is additive).
5. **The three module-level `#![allow(dead_code)]`** in `turing/src/{ir,compiler,expand}.rs` are CONSUMED or NARROWED to per-item by phase end (the audit debt); the phase's services/lints are the intended consumers — anything still dead at T8 gets a per-item allow with a stated reason or is deleted.
6. **PM-1 byte-identity** (goldens; pmt outputs); core untouched EXCEPT the zero expected — any core need is a finding first; no new deps; conventional commits, NO attribution footers; no `spec §N`/`GC{n}`/`docs/superpowers/` in comments (the audit just swept the repo clean — keep it clean); derivation-first where goldens arise; CLI tests in-process.
7. **Lint rule inventory** (spec §14, the normative lists): `.tma` = the five core rules (incl. `unused-label` per adjudication 2) + TM additions `shadowed-wildcard-rows` (a wildcard row covered by an earlier same-band row — reuse the dead_rows cover logic READ-ONLY), `retx-exit-bounds` (a `retx #k` whose k ≥ the owning function's `.frame` exit count — resolvable only when the frame is in-file; cross-file = skip, note), `rept-var-unused` (macro hygiene: a `.rept` var never substituted). `.tmc` = `leftover-debugger`, the unused family (`unused-import`/`-routine` already compile warnings — the LINT surface re-exposes them under allow-control per the pmt convention — plus the deferred `unused-graph`/`-binding`/`-graft-instance` LANDING HERE per the recorded deferral), `dead-rule` (order-aware shadowing = the same-band cover, source-level), `deprecated-call` (via docs), `redundant-identity-pairs` (a `with map` listing `x->x` pairs an identity would give), `binding-product-threshold` (mirror the compile warning as an allow-controlled lint), `writes-through-collapse` (a call/graft whose one-way (`=>`) collapsed callee symbol is provably WRITTEN by the callee — the Resolved-level data suffices, facts §9 verified), and opt-in `state-may-trap` (no catch-all — off by default, the spec's totality lint).
8. **LSP surfaces** (spec §15): `.tmc` service — live diagnostics (staged analysis + lint), completions (keywords, state names in transition position, routine/graph/bind targets in call/graft position, ALPHABET GLYPHS in pattern/write/map positions — the tape's alphabet known from context), go-to-definition (states, graft instances → graph definitions, routines, alphabets, use paths), hover (signatures with tape params/alphabets, resolved bind bindings, doc/deprecation callouts — pmc-0.3 parity), semantic tokens, quickfixes (state stub from unresolved goto; missing map pair from the mapping-legality error), formatting (the T4 formatter). `.tma` service — pma parity (no hover, operand hints in completion detail) + table-label go-to-definition from `mtc`/`djmp`/`call.m` + `.frame` field diagnostics. Single-file for now; the manifest arc's LSP overlay is inherited later (dependency noted, not designed).
9. **Plugins**: mirror `editors/{vscode,jetbrains}` structure; TWO new TextMate grammars (.tmc from the parser's surface, .tma from `tm1_syntax()`), each drift-guarded by a test against its source of truth (the pmt grammar drift-guard pattern); launch `tmt lsp`; `MIN_TESTED_TMT`; sideload-only with manual-checklist READMEs; versions 0.1.0. Artifacts are NOT committed (build outputs); the release attaches them (phase 8).

## File Structure

- Create (turing-machine/src/): `lint/` (mod + rules ×~10), `fmt.rs` (or `fmt/`), `lsp/` (tmc service) + `lsp/tma/`, `completions/` (registry + zsh), `config.rs` (tmt.json); `tests/`: lint/fmt/lsp/completions/config batteries
- Create: `editors/vscode-tm/`, `editors/jetbrains-tm/` (names: implementer judgment mirroring the pmt pair's conventions)
- Modify: `compiler.rs` (analyze_staged), `cli/{mod,lint,fmt,lsp,completions}.rs` (new subcommand files mirroring pmt's), `lib.rs`, USAGE/`--version` untouched except new subcommand rows

---

### Task 1: `analyze_staged` + the dead-code narrowing
The staged seam per GC4 (mirror pmc's `analyze_staged`/`StagedAnalysis` shape adapted to the TM `Analysis` — facts §4/§9 note the shape differences: rich `Resolved`, docs on `Resolved.docs`); never-panic property test over arbitrary input; `compile()` unchanged (assert: a compile-vs-staged agreement test on both valid and each-stage-broken inputs). Then the GC5 debt: consume what the seam consumes; narrow the rest per-item with reasons.
- [ ] TDD → implement → gate. Commit: `feat(turing-machine): staged analysis — the language-service substrate`

### Task 2: `.tmc` lint + `tmt.json` + `tmt lint`
The GC7 `.tmc` inventory as a rule layer over `analyze()`'s output (the pmt lint architecture: rules read AnalysisOutput-equivalents + ScopeSummary-equivalents from `Resolved`); the allow namespace; `config.rs` (tmt.json: nearest-ancestor, `lint.allow`, union semantics — pmt.json twin); `cli/lint.rs` (both languages by extension + dirs-and-files positionals per the pmt shape — .tma dispatch arrives in T3, stub the extension routing now); per-rule fixture batteries incl. the deferred unused-family landing and writes-through-collapse (a positive + a sound-negative: a one-way collapse whose callee never writes the symbol stays quiet).
- [ ] TDD → implement → gate. Commit: `feat(turing-machine): .tmc lint and tmt.json`

### Task 3: `.tma` lint additions + `tmt fmt` (.tma wiring)
Per adjudication 1 (turing-side merge) + GC7's `.tma` additions; the retx-exit-bounds in-file resolution; `cli/fmt.rs` (both languages, stdin `-` with `--lang`; the .tma side = `format_asm_with(src, tm1_syntax().caps)` wiring + idempotence fixtures over the frames/tables surface).
- [ ] TDD → implement → gate. Commit: `feat(turing-machine): .tma lint additions and fmt wiring`

### Task 4: the `.tmc` formatter
The phase's biggest single build (facts §3: pmc's CST-driven fmt is the template): canonical, idempotent, whitespace-only, trivia-preserving; the STATE-BLOCK GRID (within a state: align the `->`s, the `write`/`move` keywords, and the terminators — a transition table should read as a table; blank-line and comment handling per pmc's own-line rules); one-binding-arg-per-line above a width threshold (pick, document); doc/attention lines and `[deprecated]` placement preserved. Battery: idempotence + no-token-loss on EVERY 6a fixture (the six A examples, nested_graft, std.tmc itself — formatting the stdlib is the acceptance test: the formatted std.tmc should be committed if it differs, as the canonical form... judge: reformat std.tmc in this task if the formatter changes it, as its own commit, keeping the goldens green).
- [ ] TDD → implement → gate. Commits: `feat(turing-machine): the .tmc formatter` (+ `style: std.tmc in canonical form` if applicable)

### Task 5: `tmt completions`
The registry (all 9 subcommands: compile/asm/link/dis/run/tape/ir/lint/fmt + completions itself + lsp — count from cli/mod.rs; flags incl. `--call-mech`'s value set, `--entry`, `--foutline`, `--nostdlib`, `--emit-ir[=STAGE]`'s equals-only-optional, `--fno-<pass>`'s suffix family from `pass_names()`); the zsh renderer; BOTH drift guards adapted (pass-name cross-check incl. `outline`; the parser-probe over every registry entry).
- [ ] TDD → implement → gate. Commit: `feat(turing-machine): tmt completions — the zsh registry`

### Task 6: the `.tmc` LanguageService
Per GC8's `.tmc` list over the T1 staged seam + T2 lint + T4 fmt; the completion contexts are the design core (position classification over the CST: pattern/write/map cells → the contextual tape's alphabet glyphs; transition position → states; call/graft/bind target → routines/graphs/binds); hover parity with pmc 0.3; quickfixes; semantic tokens; the service-level test battery mirroring pmc's (in-process protocol tests via the core fake-transport harness — find how pmc's service tests drive it).
- [ ] TDD → implement → gate. Commit: `feat(turing-machine): the .tmc language service`

### Task 7: the `.tma` LanguageService + `tmt lsp`
The pma-parity service + the TM extras (table-label go-to-def from `mtc`/`djmp`/`call.m` operands; `.frame`/`.map`/`.exits` field diagnostics via the asm CST); `cli/lsp.rs` wiring both services through the core multi-service loop (per-URI routing by extension; capability merge — the pmt twin); end-to-end protocol tests (open a .tmc and a .tma doc in one session, each routed correctly).
- [ ] TDD → implement → gate. Commit: `feat(turing-machine): the .tma language service and tmt lsp`

### Task 8: the plugin pair + the phase milestone
Per GC9: the two grammars (drift-guarded), `editors/vscode-tm/` + `editors/jetbrains-tm/` mirroring the pmt pair (manifests, launch configs pointing at `tmt lsp`, sideload READMEs with the manual checklists, `MIN_TESTED_TMT`); BUILD both artifacts (npm package / gradle buildPlugin — the toolchains live under editors/ only) and report sizes (artifacts gitignored); the milestone = the full workspace gate + both artifact builds green + the checklists written; a hand transcript (tmt lint/fmt/completions on real files; an lsp smoke via the test harness) in the report.
- [ ] TDD → implement → gate. Commit: `feat(editors): the TM plugin pair — grammars, sideload, MIN_TESTED_TMT`

---

## Self-review notes
- Spec §14 coverage ✅ T2/T3 (every named rule mapped; state-may-trap opt-in; the deferred unused-family lands); §15 ✅ T6/T7/T8 (single-file scope + the overlay dependency noted). The §16 tooling bullets (fmt idempotency, completions drift guards, plugin manual checklists) ✅ T4/T5/T8.
- The audit debts land: dead-code allows (T1/T8 check), the deferred warnings (T2), the durable records stay accurate.
- Risks: T4's grid is the aesthetic-judgment task (fixtures lock it); T6's completion-context classification is the deepest logic (the CST gives spans — the classifier is new); T8's gradle build environment (the pmt precedent worked — JAVA_HOME from a JetBrains JBR per CLAUDE.md).
