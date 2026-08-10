# Declared Write Contracts + Stdlib Volatile Twins Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** `.tmc` signature tape parameters gain optional `writes {…}` / `preserves {…}` contract clauses checked against the footprint inference at compile time; the stdlib adopts the contracts its doc lines already promise and gains the volatile twin namespaces (`std::binaryNumbersVolatile`, `std::binaryNumbersBareVolatile`).

**Architecture:** The contract is an assertion OVER the inference, never a replacement (spec §6.2's principle, restated ref-free: inside a compilation unit the inference is at least as precise, so lints and passes keep consuming the inference; the declaration's value is that it breaks the build when a body edit violates the promise). The grammar work mirrors the volatile round's sweep order (reserve → parse → resolve/check → fmt → lint → LSP → stdlib → docs); the checker is a new post-`check_worlds` stage consuming `footprint::infer_resolved`; the twins are pure `.tmc` source additions to `std.tmc` with three pinned test families. This is plan 5 of the volatile/async/footprint round — the spec is `docs/superpowers/specs/2026-08-08-volatile-async-footprint-design.md` §6; the round rulings live in the controller's memory and are restated inline below wherever they bind.

**Tech Stack:** Rust workspace, crates/turing-machine only. No new dependencies.

## Global Constraints

- **PM and core untouched**: `git diff --stat -- crates/post-machine crates/core` EMPTY at every task boundary and at the end. (Diagnostics types `Fix`/`Edit`/`Applicability` already live in core — consumed, never modified.)
- **Version spaces: NOTHING moves.** `TMC_LANG_VERSION` stays 0.1 (unreleased space — the grammar amendment rides in place; `docs/tmt/language.md`'s version-history line for 0.1 absorbs the clauses). `TM_IR_VERSION` stays 2 (`ir.rs:62`) — contracts are IR-inert by design; no `ir.rs` change of any kind. `serde_tags_are_frozen` untouched. Container formats untouched. Manifest schemas untouched.
- **RESERVED 25 → 27** (`writes`, `preserves`). Every count string moves in ONE commit: `lexer.rs:9`, `lexer.rs:16`, the `[&str; 25]` at `lexer.rs:23`, the test doc `lexer.rs:634` and `assert_eq!(RESERVED.len(), 25)` at `lexer.rs:639`, `parser.rs:9`, `docs/tmt/language.md:824` ("Twenty-five" → "Twenty-seven") + the fenced table `:827-831`, and the tmc TextMate grammar (a NEW `keyword*`-prefixed rule — see Task 1). The `editor_grammar.rs` set-equality guard and the lexer transparency test enforce both directions.
- **Fixed clause order is a GRAMMAR rule, not an fmt rule**: the parser accepts only `writes` before `preserves`. Rationale (binding): fmt is whitespace-only and token-preserving (`formatting_never_changes_a_token`, `tests/fmt_tmc.rs:97`) — an order-flexible grammar that fmt "canonicalizes" would require token reordering, which the fmt contract forbids. Reversed order is a targeted parse error.
- **The checker consumes the engine read-only**: `crates/turing-machine/src/footprint.rs` is NOT modified by this plan. `infer_resolved(&Resolved)` (`footprint.rs:601`) and `SymSet` are `pub(crate)` in the same crate — consumed as-is. Over-approximation soundness carries over: inferred ⊇ actual means a passing containment check is a real guarantee; a FAILING check on slack-free code is impossible only up to inference precision, and the error message never claims the body "does" write — it says "may write".
- **Standing lessons from plan 4 (binding on every task)**: (a) never skip a conservative branch with "that wouldn't compile anyway" — `analyze` succeeds on plenty that `expand`/`lower` reject, and lints/LSP run on exactly that source; (b) an acceptance claim about a `.tmc` program is established by compile AND link, never compile alone (duplicate validators).
- **Stdlib edit tripwires** (Tasks 7-8 hit ALL of these deliberately — updating them is in-scope, silently breaking them is not): `tests/fmt_tmc.rs:117` (`std.tmc` must be byte-canonical — write clauses in canonical form from the start), `src/stdlib/mod.rs:352` (docs-count assert), `mod.rs:283-298` + `:365` (the hand-written 14-name roster + `len()==14`), `mod.rs:363-371` (declaration lines pure ASCII), the 23-test `stdlib_golden.rs` matrix, and `opt_equivalence.rs:1134` (stdlib O0==O1 do-no-harm).
- **Hover/doc transcript ripple**: stdlib adoption (Task 7) changes std signature heads, which are quoted in full-string LSP test pins (`lsp/tests.rs`) and in `docs/lsp.md`/`docs/tmt/lint.md`/`docs/tmt/stdlib.md` transcripts. Task 7 re-runs and updates every quote it moves — a stale quoted transcript is a task failure, not a docs-task-later item.
- Published-content policy: no issue/PR numbers, no "Task N", no "spec §", no superpowers paths in code comments or docs pages. Doc comments cite durable pages (`docs/tmt/language.md (contracts)` style).
- Conventional commits with scope. Never `--amend`; fix rounds are new commits. Goldens derivation-first.
- Gates for every task, FOREGROUND: `cargo test -p mtc-turing-machine`, `cargo fmt --check`, `cargo clippy -p mtc-turing-machine --all-targets -- -D warnings`; final gates add the core/PM suites and the no_std build.

---

### Task 1: Reserve `writes` and `preserves` — keywords 26 and 27

**Files:**
- Modify: `crates/turing-machine/src/lexer.rs` (array `:23-48`, doc counts `:9`, `:16`, test `:634-653`), `crates/turing-machine/src/parser.rs` (module-doc count `:9`), `crates/turing-machine/src/parser/tests.rs` (reservation coverage), `docs/tmt/language.md` (`:824-831` count word + table reflow), `editors/grammars/tmc.tmLanguage.json` (new rule)
- Test: existing guards — `lexer.rs:638` transparency test, `tests/editor_grammar.rs:62` set-equality

Mirror the volatile round's reservation commit exactly (it changed the same file set). Grammar: add a NEW repository rule `keywordsContract` with `"match": "\\b(writes|preserves)\\b"`, painted `keyword.control.tmc`, added to the includes list — do NOT extend `keywordsModifier` (these are clause keywords, not declaration modifiers; the guard only requires every `keyword*` rule to be a plain alternation and the union to equal `RESERVED`).

- [ ] **Step 1**: Add both words to `RESERVED` (after `volatile`), update all five in-crate count strings and the two doc counts, flip `assert_eq!(RESERVED.len(), 25)` to 27.
- [ ] **Step 2**: `parser/tests.rs` — extend the reservation coverage the volatile follow-up added: `writes` and `preserves` each rejected as an identifier position name (mirror the existing `volatile`-as-name case).
- [ ] **Step 3**: `docs/tmt/language.md` — "Twenty-seven words are fully reserved"; reflow the fenced table (the last row currently holds `volatile` alone — reflow to keep 8 columns per row, last row `volatile writes preserves`).
- [ ] **Step 4**: The grammar rule + includes entry. Run `cargo test -p mtc-turing-machine --test editor_grammar` — both directions must pass.
- [ ] **Step 5**: Full gates. Commit: `feat(turing-machine): reserve writes and preserves — keywords 26 and 27`.

---

### Task 2: Parse the clauses

**Files:**
- Modify: `crates/turing-machine/src/parser.rs` (`SigParamKind` `:149-156`, `sig_param` `:1521-1563`), `crates/turing-machine/src/parser/tests.rs`
- (No `cst.rs` change: `Signature`/`SigParam` are shared by AST and CST — one representation.)

**Interfaces (produces — later tasks consume these exact shapes):**

```rust
// parser.rs — new type beside Signature/SigParam:
/// One contract clause on a signature tape parameter: the keyword's span,
/// the brace-set's elements (same element grammar as an alphabet body:
/// singles and ranges), and the whole clause's span (keyword → `}`).
pub struct ContractClause {
    pub elems: Vec<AlphabetElem>,
    pub kw_span: Span,
    pub span: Span,
}

// SigParamKind::Tape grows two fields (both None when absent):
Tape {
    alphabet: String,
    alphabet_span: Span,
    volatile: bool,
    writes: Option<ContractClause>,
    preserves: Option<ContractClause>,
},
```

**Grammar (pinned):** after the alphabet name (`sig_param` line 1539), optionally `writes { ELEMS }`, then optionally `preserves { ELEMS }`. `ELEMS` = comma-separated `alphabet_elem()` list, EMPTY allowed (`writes {}` means "writes nothing" — a meaningful upper bound; `preserves {}` is legal and inert). Fixed order enforced by the parser: after a `preserves` clause, a following `writes` keyword is the targeted error `` `writes` must come before `preserves` ``; a duplicate clause keyword is `` duplicate `writes` clause `` (same shape for preserves). Clauses are signature-only: `parse_tape` (machine tape declarations, `:1665`) does NOT accept them — a clause keyword there falls out as the existing expected-token error, pinned by a negative test. On the `State` param kind a clause keyword is likewise a parse error. `SigParam.span` (`:1548`) extends to the last clause's closing `}`.

- [ ] **Step 1: Failing tests** in `parser/tests.rs`: a param with both clauses round-trips into the expected `ContractClause` shapes (spans included); `writes {}` parses empty; ranges parse (`'a'..'c'` — whatever `alphabet_elem`'s range spelling is, copy it from an alphabet-body test); reversed order errors with the pinned message; duplicate clause errors; clause on a machine tape decl errors; clause on a `state` param errors; `SigParam.span` covers the clause.
- [ ] **Step 2**: Red.
- [ ] **Step 3**: Implement — a `contract_clause(kw: &str)` helper mirroring `parse_alphabet`'s body loop (`:1358-1374`: `expect(LBrace)`, comma loop over `alphabet_elem()`, `expected "`,` or `}`"`). Keep `interior_comments` OUT of clause bodies (clauses are one-line constructs; a comment inside one is the known fmt-relocation exception — note this in a code comment).
- [ ] **Step 4**: Green; full gates (fmt is not yet exercised on clause sources — no committed fixture carries one until Task 7).
- [ ] **Step 5**: Commit `feat(turing-machine): parse writes/preserves contract clauses on signature tape params`.

---

### Task 3: Resolve, the two errors, and the containment check

**Files:**
- Modify: `crates/turing-machine/src/compiler.rs` (`ResolvedTape` `:841-850`, `resolve_world` sig arm `:1521-1533`, the error enum + `code_registry!` `:284-341` + `Display` `:364+`, a new check after `resolve_program`'s `:961`), `docs/tmt/cli.md` ("### Compile errors" table — the `error_code_docs` guard forces the two rows)
- Test: `compiler.rs` in-file, `crates/turing-machine/tests/cli_programs.rs`

**Interfaces:**
- Consumes: `crate::footprint::{infer_resolved, FootprintTable, SymSet}` (read-only), `ContractClause` from Task 2.
- Produces: `ResolvedTape` grows `writes: Option<SymSet>`, `preserves: Option<SymSet>` (index sets in the tape's own alphabet frame; `None` = clause absent; machine-world tapes always `None`). Two `CompileErrorKind` variants with codes `contract-symbol-unknown` and `writes-outside-contract` (registry 56 → 58 rows).

**Semantics (pinned):**
- Resolution: each clause element resolves against the param's alphabet glyphs; ranges expand exactly as alphabet bodies expand them; an unknown glyph raises `ContractSymbolUnknown` at that element's span. Display: `` '{glyph}' in the `{clause}` clause is not a symbol of alphabet `{alphabet}` `` (clause = `writes`|`preserves`).
- The check: a new `check_contracts(resolved)` invoked in `resolve_program` AFTER `check_worlds` (`:961`) and before `unused_import_warnings`. It runs `infer_resolved` ONCE (only when at least one world carries a clause — note the cost gate in a comment), then per contracted tape: effective allowed = (`writes` if present else `SymSet::full(cardinality)`) minus `preserves`; if inferred ⊄ allowed, raise `WritesOutsideContract` at the PARAM's span. Display: `` `{world}` may write {glyphs} on tape `{tape}`, which its contract forbids `` — offending glyphs = inferred ∖ allowed, rendered single-quoted comma+space ascending (the footprint hover's glyph-render convention). "may write", never "writes" — the inference over-approximates.
- Overlap (a symbol in both clauses) is NOT an error — redundancy, `preserves` wins by the subtraction above; the lint is Task 5's.
- IR-inert: no `ir.rs` change; the existing frozen-serde and IR round-trip suites prove it by not moving.

- [ ] **Step 1: Failing tests**: in-file — a satisfied contract compiles (`writes {'0','1'}` over a body writing only `'1'`); a violated `writes` errors with the pinned message and code; a violated `preserves` errors (body writes a preserved glyph); the effective-set rule (symbol in both clauses: body writing it ERRORS — preserves wins); `writes {}` over a non-writing body compiles, over a writing body errors; unknown glyph in either clause errors with `contract-symbol-unknown`; a contract on a GRAPH param is checked too (graphs are inferred worlds); transitive violation (the body's CALL writes the forbidden glyph through a map — the inference is transitive, so the contract sees it). CLI-route: `cli_programs.rs` — a violating file exits with the compile-error path and the `[writes-outside-contract]` code visible.
- [ ] **Step 2**: Red.
- [ ] **Step 3**: Implement (variants → registry rows → Display arms → resolution in the sig arm → `check_contracts`). Add the two rows to `docs/tmt/cli.md`'s compile-error table; run `cargo test -p mtc-turing-machine --test error_code_docs` to prove the guard sees them.
- [ ] **Step 4**: Self-mutation before declaring done: neutralize the subtraction (allowed = writes only, preserves ignored) → the preserves-violation test dies; make the check use the DECLARED set as the inference (assert declared ⊆ declared) → every violation test dies. Quote kills in the report.
- [ ] **Step 5**: Green; full gates. Commit `feat(turing-machine): resolve contract clauses and check them against the inferred footprint`.

---

### Task 4: fmt canonicalization

**Files:**
- Modify: `crates/turing-machine/src/fmt.rs` (`signature_params` `:714-727`)
- Test: `crates/turing-machine/tests/fmt_tmc.rs` (in-file fixture strings, the volatile round's pattern), `crates/turing-machine/src/fmt/tests.rs` if unit-level cases fit better

**Canonical form (pinned):** ` writes { '0', '1' } preserves { '#' }` — single space before each keyword, alphabet-body brace spacing (`{ elems }` with `", "` separators — `fmt.rs:1224`'s convention), elements re-encoded losslessly via `alphabet_elem_text` (`fmt.rs:458`). Empty set renders `writes {}` (no inner space — mirror however the alphabet formatter renders an empty body; if alphabets can't be empty, pin `{}` explicitly with a comment). The clause string joins the existing `format!("{prefix}tape {}: {alphabet}")` entry at `fmt.rs:722`. A long clause list may push the entry past `LINE_WIDTH` — `paren_list` already breaks one-param-per-line; no intra-clause wrapping (pin with a test using a wide clause).

- [ ] **Step 1: Failing tests**: canonical form (messy spacing in → canonical out); idempotence on a clause-bearing source; token preservation (`formatting_never_changes_a_token` pattern on a clause fixture); both clauses + volatile prefix compose (`volatile tape num: symbols writes { '0' }`); the wide-clause line-break behavior.
- [ ] **Step 2**: Red → implement → green.
- [ ] **Step 3**: Full gates. Commit `feat(turing-machine): fmt renders contract clauses canonically`.

---

### Task 5: the `contract-clause-overlap` lint

**Files:**
- Create: `crates/turing-machine/src/lint/rules/contract_clause_overlap.rs`
- Modify: `crates/turing-machine/src/lint/rules/mod.rs` (module line, alphabetical), `crates/turing-machine/src/lint/mod.rs` (`RULES` — default-on, 16 → 17 entries), `crates/turing-machine/src/completions/registry.rs` (ONE comment line — see Step 4)
- Test: in-file + `crates/turing-machine/tests/lint_programs.rs`

**Semantics (pinned):** for every signature tape param carrying BOTH clauses, every symbol present in both sets is redundant (`preserves` wins in the checker's subtraction). One finding per overlapping SOURCE ELEMENT in the `writes` clause: `code: "contract-clause-overlap"`, span = that element's span in the `writes` clause, message: `` '{glyph}' is in both `writes` and `preserves`; `preserves` wins, so the `writes` entry is inert ``. For a RANGE element that only partially overlaps, the finding names the overlapping glyphs and carries NO fix (splitting a range is not a whitespace-safe single edit); a fully-covered element gets the fix.
**Quickfix (pinned):** remove the element from the `writes` set — description `` remove '{glyph}' from the `writes` clause ``, `Applicability::MachineApplicable`, edits = the element plus its adjacent comma (leading comma if last element, trailing if not — copy the list-element-deletion span discipline from the existing deletion quickfixes, e.g. `unused_alphabet`'s). Removing the LAST element leaves `writes {}` — which is a DIFFERENT contract (writes-nothing) than deleting the clause; when the element is the only one, the fix instead removes the whole clause (kw_span → `}`), description `` remove the emptied `writes` clause ``. Source-level spans come from the AST clause (`ctx.program` view — the `unused-exit` precedent at `lint/mod.rs:96-97`).

- [ ] **Step 1: Failing tests** (in-file, `findings(src)` helper filtered on the code): single overlap reported at the writes-element span with the pinned message; no overlap → clean; overlap via a range (range in writes, single in preserves) → finding, no fix; fix on a middle element removes element+comma; fix on the only element removes the whole clause; applied fix re-lints clean AND the recompiled object is byte-identical (the contract subtraction already ignored the removed entry — that IS the redundancy claim; use the `lint_programs.rs:614-625` compile-and-compare pattern).
- [ ] **Step 2**: Red (module absent) → implement → green. The rule needs no footprint call — it compares the two DECLARED sets only (note this in the module head: cheap, purely syntactic-plus-resolution).
- [ ] **Step 3**: Integration (`lint_programs.rs`, the dead-map-pair banner pattern): dirty file exits 1; `--allow contract-clause-overlap` suppresses (auto-union — assert it); the fix corpus test.
- [ ] **Step 4**: One-line comment correction in `completions/registry.rs:921-922`: the assertion's rationale "(no TM-1 rule emits a fix)" is false (several rules emit `Fix`es; they surface through LSP code actions, not a CLI `--fix`). Reword the parenthetical to `(fixes surface through LSP code actions; the CLI has no --fix)`. Controller-authorized scope extension; the assertion itself stays.
- [ ] **Step 5**: Full gates. Commit `feat(turing-machine): the contract-clause-overlap lint with the element-removal quickfix`.

---

### Task 6: LSP — hover renders the clauses; completions pinned deferred

**Files:**
- Modify: `crates/turing-machine/src/lsp/navigate.rs` (`world_head` `:747-770`, `Target::Tape` arm `:682-688`, `sig_tapes` `:142-164` + the `WorldView` tape tuple `:71-83`)
- Test: `crates/turing-machine/src/lsp/tests.rs`

**Rendering (pinned):** both independent renderers gain the clauses in fmt's canonical spelling (reuse the fmt helper if it can be called from here without visibility contortions — `pub(crate)` it if needed; otherwise mirror the string and add a two-way drift test between the two renderings of one fixture). `world_head`: `routine r(tape num: bits writes { '0', '1' })`. `Target::Tape` hover: `tape num: bits writes { '0', '1' }` (after the volatile prefix if both). The `WorldView` tape tuple becomes a small struct (name, name_span, alphabet, alphabet_span, volatile, writes, preserves) — the 5-tuple is at its limit; a struct is the smaller diff across the four touch points.
**Reader-space note (goes in a comment + Task 9's docs):** a hovered world can now show BOTH `writes` surfaces — the DECLARED clause inside the signature head and the INFERRED `writes <tape>: {…}` block below it. They are different statements (promise vs computation) and can legitimately differ (slack). The docs page owns explaining this; the code comment just flags the adjacency.
**Completions:** clause-position completions need a new signature-frame context in `lsp/context.rs` — DEFERRED, mirroring the volatile round's pinned-negative precedent (`lsp/tests.rs:1063` `after_volatile_the_item_boundary_completion_offers_nothing_yet`). Add the analogous pin: after `tape num: bits ▮` inside a signature, completion offers nothing yet, with the same "a small feature of its own" comment.

- [ ] **Step 1: Failing tests**: `hovering_a_routine_shows_its_declared_contract` (full-string pin: head with clause, inferred writes block below, doc body last — extends the Task-4-of-plan-4 placement pin); `tape_hover_shows_the_clause`; the declared-vs-inferred divergence case (declared `writes { '0', '1' }`, body writes only `'0'` → head shows the declaration, inferred block shows `{'0'}` — both true); the completions pinned-negative.
- [ ] **Step 2**: Red → implement → green.
- [ ] **Step 3**: Full gates. Commit `feat(turing-machine): hover renders declared contract clauses in signature heads`.

---

### Task 7: stdlib `preserves`/`writes {}` adoption

**Files:**
- Modify: `crates/turing-machine/src/stdlib/std.tmc`, `crates/turing-machine/src/lsp/tests.rs` (quoted-head pins), `docs/lsp.md` + `docs/tmt/lint.md` + `docs/tmt/stdlib.md` (ONLY the transcripts/quotes this task's source edit moves — full docs are Task 9)
- Test: `crates/turing-machine/tests/` — a new codegen-inert proof; every existing stdlib gate re-run

**Adoption set (pinned — clauses go on BOTH the graph and its facade, so the checker validates the graph's own inference AND the facade's transitive one):**
- `std::binaryNumbersBare::invertNumberGraph` + `invertNumber`: `preserves { '_' }` — the load-bearing marker-survival claim (`std.tmc:247`) becomes machine-checked.
- The four unconditional walkers (`goToNumber`, `goToNumbersStart`, `goToNextNumber`, `goToPreviousNumber`, each + its graph): `writes {}` — "the tape is unchanged" is exactly the writes-nothing contract, and the plan-4 inference proves it (their graphs move and read, never write). Eight clause sites.
- Conditional claims (`deleteNumber`, `normalizeNumber`, `plusOne`, `minusOneFast`, bare `minusOne` — "a head not on a number leaves the tape untouched") are NOT expressible as a write-set clause and are deliberately NOT adopted; a one-line note in the plan report records this so Task 9's docs say it honestly.
- Doc lines stay: the `?` prose and the clause now state the same fact twice in different registers; the clause is the checked one. Do NOT reword doc prose in this task.

**Canonical-form discipline:** clauses are written in fmt's canonical spelling directly (the dogfood gate `fmt_tmc.rs:117` requires `std.tmc` byte-canonical); ASCII only (`mod.rs:363` guard); declaration lines grow but stay one-line.

- [ ] **Step 1**: Write the failing codegen-inert proof FIRST: `tests/` — compile a fixture pair (same source ± clauses) → `object.to_bytes()` byte-identical (the `lint_programs.rs:614` pattern, but as its own named test beside the stdlib gates: `contracts_are_codegen_inert`). Also assert the stdlib's own O0==O1 do-no-harm still holds after adoption (it re-runs anyway in `opt_equivalence.rs:1134`; reference it in the report, don't duplicate it).
- [ ] **Step 2**: Edit `std.tmc` (ten clause sites). Run the checker — the adoption must compile CLEAN on the first try; a `writes-outside-contract` here means either the inference or the doc line is wrong: STOP and report BLOCKED with the finding verbatim (do not weaken a clause to pass).
- [ ] **Step 3**: Sweep the moved quotes: `lsp/tests.rs` full-string hover pins that quote bare `invertNumber`'s head (now carries ` preserves { '_' }`); `docs/lsp.md`'s hover transcript; `docs/tmt/lint.md`'s dead-map-pair worked example IF its transcript shows a std head; `docs/tmt/stdlib.md`'s anatomy example (`invertNumberGraph` signature at `:143-188`). Re-run every one against the real tools before pasting.
- [ ] **Step 4**: Full stdlib gates: `fmt_tmc` (canonical + idempotent), `stdlib_golden` (all 138 combos), `stdlib/mod.rs` unit tests (counts unchanged: roster stays 14, docs stay 28 — adoption adds no entities), `facade_proof`, full crate suite.
- [ ] **Step 5**: Commit `feat(turing-machine): the stdlib declares the contracts its doc lines promise`.

---

### Task 8: the volatile twin namespaces

**Files:**
- Modify: `crates/turing-machine/src/stdlib/std.tmc` (two new namespaces), `crates/turing-machine/src/stdlib/mod.rs` (the roster/docs-count guards — deliberate updates), `docs/tmt/stdlib.md` (the twins section — this task owns it; Task 9 only cross-links)
- Test: `crates/turing-machine/src/stdlib/mod.rs` in-file (family 1), `crates/turing-machine/tests/stdlib_golden.rs` or a sibling `tests/stdlib_twins.rs` (families 2-3)

**What (round ruling, restated):** the volatile twins exist so the CALLEE side of a std call carries the volatile mark: the stdlib arrives at link time compiled once, and a compiler-side splice never protects a call into it — so value-assuming passes (a future optimizer round) need twin facades whose signature tapes are flagged TODAY, with bodies byte-identical until such passes exist. Namespace twins, not per-routine suffixes:
- `std::binaryNumbersVolatile` — 10 routines, same names as `std::binaryNumbers`, each `export routine <op>(volatile tape num: symbols)`.
- `std::binaryNumbersBareVolatile` — 4 routines mirroring `std::binaryNumbersBare`.
- Graph-backed facades graft the SAME shared graphs (the originals in the plain namespaces — no graph duplication; graft rows land on the twin's flagged tape, and grafts are host-governed, so the flag takes effect at every splice).
- The two call-chain routines (delimited `invertNumber`, `minusOne`) mirror their bodies with every `call` retargeted to the TWIN of its callee (volatility held across the whole chain; a twin calling a plain routine would drop the mark at the boundary — the exact foot-gun the twins exist to close).
- Contracts mirror Task 7's adoption: a twin's clauses equal its plain counterpart's (family 1 asserts this).
- Each twin routine gets a one-line `?` doc: same contract as the plain counterpart, tape treated as volatile. (Doc-count guard moves: +14 documented routines.)

**The three test families (round ruling — all three, pinned):**
1. **Roster-mirror drift guard** (in `stdlib/mod.rs` tests): set-compare the twin namespaces' exports against the plain ones — same routine names, same arities, same sig shapes MODULO the volatile mark, same contract clauses; every twin tape param IS flagged volatile; every graph-backed twin facade grafts the SAME graph as its plain counterpart (compare graft targets through `analyze`'s `Resolved` — `grafts` on `ResolvedWorld`). Update the hand-written roster test to 28 names and the docs-count assert (28 → 42: +14 twin routines).
2. **Functional equivalence corpus**: per twin pair, the plain golden's seed through a twin consumer produces the SAME final snapshot and outcome as the plain routine — reuse `stdlib_golden.rs`'s seeds and its `assert_stdlib_golden` matrix shape (O0/O1 × mono/frames/hybrid). Tacts are free to diverge once value-assuming passes land; assert tapes and outcome only.
3. **Today-only byte-identity pin**: per twin pair, the compiled stdlib object's code blob for the twin routine is byte-identical to the plain routine's blob (probe the exact per-symbol code accessor on `ObjectFile` in Step 1; names live in the symbol table, not the blobs). The test carries the obligation comment verbatim: `// TODAY-ONLY: the first optimizer pass that assumes values on non-volatile tapes MUST flip this test to functional-equivalence-only (family 2 stays; this byte pin goes) and say so in its release notes.`

- [ ] **Step 1: Probe** (record answers in the report): the in-file cross-namespace graft/call spelling `std.tmc` needs (how the twin namespace references `std::binaryNumbers::goToNumberGraph` — qualified target vs `use`; follow whatever the delimited↔bare cross-representation calls already do at `std.tmc:250-277`); the per-symbol code-blob accessor for family 3.
- [ ] **Step 2: Write families 1-3 as failing tests** (family 1 fails on the missing namespaces; families 2-3 on missing routines).
- [ ] **Step 3**: Write the twins in `std.tmc` (canonical form, ASCII, one-line declarations). Compile clean, all three families green.
- [ ] **Step 4**: Update the deliberate guard moves: roster 14 → 28 (explicit names), docs count 28 → 42, `docs/tmt/stdlib.md` twins section (why they exist — the link-time rationale in ref-free prose — the naming rule, the same-graph guarantee, the byte-identity-today status).
- [ ] **Step 5**: Full stdlib gates (fmt canonical, goldens, facade_proof, opt_equivalence O0==O1 now over the twinned source). Full crate gates. Commit `feat(turing-machine): the stdlib volatile twin namespaces`.

---

### Task 9: docs

**Files:**
- Modify: `docs/tmt/language.md` (the contracts section), `docs/tmt/lint.md` (`### contract-clause-overlap`), `docs/tmt/stdlib.md` (adoption notes in the roster tables; cross-link the twins section Task 8 wrote), `docs/tmt/fmt.md` (one canonical-form line in the signature-rendering area), `docs/lsp.md` (the declared-vs-inferred hover distinction, if Task 7's quote sweep didn't already land it)

**Content (pinned):**
- `language.md`: a `### Contract clauses` section BESIDE `### Volatile tapes` (`:219`): syntax in canonical form, both-optional + fixed order + why the order is grammar (token-preserving fmt), empty-set meaning (`writes {}` = writes nothing), the effective-set rule ((writes ∨ full) − preserves), the one check against the INFERRED footprint with the "may write" over-approximation caveat, the two error codes by name, overlap → the lint by name, signature-params-only (machine tape declarations don't carry clauses), legal on graph and routine params. Grammar-version history line: the 0.1 entry absorbs the clauses (unreleased space).
- `lint.md`: the rule section in RULES order (after `writes-through-collapse`'s `:335` region, before the opt-in pair) — message and fix-description quoted byte-for-byte from the rule file, the range-partial-overlap no-fix case, the only-element whole-clause-removal case, a run-first transcript.
- `stdlib.md`: the roster tables gain a Contract column or per-row note for the ten adopted sites; the "fourteen of them" count sentence (`:188`) becomes twenty-eight with a twin cross-link.
- Every transcript run first against the real binaries; policy grep; citation sweep (code comments added in Tasks 2-8 citing these pages must resolve).

- [ ] **Step 1**: Write the pages; run every transcript.
- [ ] **Step 2**: Sweep: policy grep (no refs), citation resolution, the reserved-table count (Task 1 already moved it — verify it still reads correctly beside the new section).
- [ ] **Step 3**: Full gates incl. `error_code_docs` + `cli_docs`. Commit `docs(tmt): declared write contracts — language, lint, stdlib`.

---

## Final gates (whole branch, before merge)

`cargo test -q -p mtc-core` · `cargo test -q -p mtc-post-machine` · `cargo test -q -p mtc-turing-machine` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo fmt --check` · `cargo build -p mtc-core --no-default-features` — all foreground, all exit 0. `git diff --stat -- crates/post-machine crates/core` EMPTY.

The final whole-branch review additionally checks the cross-task seams:
- The checker and the lint read the SAME resolved clause sets (one resolution in Task 3; the lint must not re-resolve glyphs itself).
- fmt's canonical clause spelling ↔ the two hover renderers ↔ every docs example: one spelling everywhere (the Task 6 drift test is the pin).
- The stdlib adoption's clauses ↔ the doc-line prose they formalize: no contradiction introduced (the conditional-claim exclusions stayed prose-only).
- Family 1's modulo-volatile comparison really ignores ONLY the volatile mark (a twin with a divergent clause must fail it — mutation).
- `RESERVED` count 27 consistent across all nine count sites.
- The error-code registry, `docs/tmt/cli.md`'s table, and `CODES` in three-way agreement (the guard runs; eyeball the prose rows).
- `TM_IR_VERSION` still 2; `ir.rs` untouched; serde tags frozen.
- The plan-4 engine untouched: `git diff <base>..HEAD -- crates/turing-machine/src/footprint.rs` EMPTY.
