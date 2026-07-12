# .pmc doc/attention lines Plan 2/2 — consumers: fmt, deprecated-call, LSP hover + tags, stdlib

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Everything that reads the plan-1 machinery: fmt prints doc runs canonically, the `deprecated-call` lint rule fires with tag-carrying LSP diagnostics, hover ships as the framework's first new capability, completion candidates carry `detail` + deprecation tags (the #25 fold-in), and the stdlib documents itself.

**Architecture:** fmt walks `FunctionCst.doc_run` (grammar fixed the order; printing is mechanical). The lint rule and both LSP features read only `Analysis.docs` (qualified-name map) — no CST walking in consumers. The LSP framework grows hover plumbing + two additive wire fields (diagnostic `tags`, completion `detail`/`tags`); `Candidate` reshapes once for both #17 and #25.

**Tech Stack:** Rust edition 2024, zero new deps. Design authority: `docs/superpowers/specs/2026-07-12-pmc-doc-lines-attributes-design.md`. Prerequisite: plan 1 merged (`FnDoc { paragraphs, attention, deprecated }`, `Analysis.docs`, `FunctionCst.doc_run`, `DocRunKind`).

## Global Constraints

- Zero new dependencies. Gates at every commit: `cargo fmt`, `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`.
- Consumers read `Analysis.docs` only — no per-consumer CST doc extraction (the walk-dedup disease).
- fmt: zero-token-changes applies to doc prose; run at the declaration's indent; one space after sigil; empty doc line prints bare `?`.
- Hover content is PLAIN TEXT (LSP `MarkupKind::PlainText`) in v1 — no markdown interpretation.
- Published docs ref-free; conventional commits; no AI/Claude attribution footers; do NOT merge or push.

## File Structure

- `crates/post-machine/src/fmt/mod.rs` — doc-run printing (Task 1).
- `crates/post-machine/src/lint/rules/deprecated_call.rs` + `lint/mod.rs` registry + `docs/lint.md` (Task 2).
- `crates/core/src/lsp/{mod,types,server}.rs` — hover trait method + wire types + handler + capability; `ServiceDiagnostic.deprecated`; `Candidate.detail`/`deprecated` (Task 3).
- `crates/post-machine/src/lsp/mod.rs` + feature modules — pmc hover, tags, detail (Task 4).
- `crates/post-machine/src/lsp/pma/{mod,complete}.rs` — pma hover:None + operand-hint detail (Task 5).
- `crates/post-machine/src/stdlib/std.pmc` + `docs/lsp.md` + editor READMEs (Task 6).

---

### Task 1: fmt prints doc runs

**Files:**
- Modify: `crates/post-machine/src/fmt/mod.rs` (+ its fixture corpus under the fmt tests / `crates/post-machine/tests/fmt_programs.rs` harness)

Printing rules (each a test): run lines print immediately above the declaration at ITS indent; canonical form is sigil + one space + verbatim text (`?text` input normalizes to `? text` — whitespace-only change); empty doc line prints `?` alone (no trailing space); attention with attr prints `! [deprecated] message` (single spaces); comments inside the run print under the existing comment rules; blank-line collapse policy unchanged; order needs no logic (grammar enforces).

**Steps:**
- [ ] Failing tests: canonical fixture (documented top-level + nested + deprecated-with-message) reprints byte-identically; scrambled-space variant normalizes TO it; empty-`?` paragraph break preserved; idempotence over the new fixtures; token-spelling guard (from the #19 fix) covers doc text verbatim (add a doc line containing `007` to prove prose is untouched).
- [ ] Implement; pass; full gates.
- [ ] Commit: `feat(post-machine): fmt prints doc and attention runs at declaration indent`

---

### Task 2: `deprecated-call` lint rule

**Files:**
- Create: `crates/post-machine/src/lint/rules/deprecated_call.rs`
- Modify: `crates/post-machine/src/lint/mod.rs` (RULES — append `("deprecated-call", …)` last), `docs/lint.md`

Rule: for every call item in the flattened AST whose resolved target name is a key in `ctx.analysis-docs-equivalent` — note `LintContext` today carries `{source, tokens, ast, scopes}`; it gains `docs: &HashMap<String, FnDoc>` (from `Analysis.docs`; the LSP's staged path already has it; `lint()`'s own path takes it from `analyze`) — with `deprecated: Some(msg)`. Span = the call's span. Message: `call to deprecated function 'NAME'` + `: MSG` when non-empty. No fix. The declaring function's own body is NOT exempt in v1? — it IS exempt only in that self-reference cannot occur (a function cannot call itself by name in .pmc? recursion is legal — do NOT exempt; a deprecated function calling itself gets flagged like any caller; simplest and honest).

**Steps:**
- [ ] Failing tests: direct call flagged with message; namespaced call flagged; call to non-deprecated documented fn NOT flagged; `--allow deprecated-call` suppresses (union validation picks the new code up mechanically — add the code to any allow-list test that enumerates); message-less `[deprecated]` renders without the colon suffix.
- [ ] `docs/lint.md`: rule entry (report-only tier), ref-free.
- [ ] Implement; pass; full gates.
- [ ] Commit: `feat(post-machine): deprecated-call lint rule`

---

### Task 3: LSP framework — hover plumbing + tag/detail wire fields

**Files:**
- Modify: `crates/core/src/lsp/mod.rs`, `crates/core/src/lsp/types.rs`, `crates/core/src/lsp/server.rs`

**Interfaces (Produces):**

```rust
// mod.rs — trait gains (no default; every impl states it):
fn hover(&mut self, uri: &str, pos: Pos) -> Option<HoverContent>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverContent { pub text: String, pub span: Span } // plain text

// ServiceDiagnostic gains: pub deprecated: bool,   // → wire tags: [2]
// Candidate gains:        pub detail: Option<String>, pub deprecated: bool, // → wire detail + tags: [1]
```

Server: `hoverProvider: true` in capabilities; `textDocument/hover` request handler routing by the mux binding like every other request; wire shape `{ contents: { kind: "plaintext", value }, range }`; `None` → `null` result. Diagnostic publishing adds `"tags":[2]` when `deprecated`; completion mapping adds `detail` and `"tags":[1]`. `FakeService` + the two server-test fixture services gain `hover` (fake returns a canned value so the handler is testable) and the new Candidate fields in their fixtures.

**Steps:**
- [ ] Failing framework tests: hover request routes to the bound service and renders the wire shape (both `Some` and `null`); a deprecated ServiceDiagnostic publishes `tags:[2]`; a Candidate with detail+deprecated reaches the wire with `detail` + `tags:[1]`; single-service byte-identity pins updated deliberately (the initialize result changes by exactly `hoverProvider:true` — update the full-JSON expectations, calling that out in the test diff).
- [ ] Implement; pass; full gates (pmc/pma services updated mechanically to compile: pmc `hover` → `None` placeholder THIS task, real impl next task; pma `hover` → `None`).
- [ ] Commit: `feat(core): lsp hover capability + deprecation tags + completion detail on the wire`

---

### Task 4: pmc service — hover, tags, detail

**Files:**
- Modify: `crates/post-machine/src/lsp/mod.rs` (+ `navigate.rs` for position→target reuse)

Behavior: `hover(uri, pos)` resolves the position exactly like `definition` does (REUSE navigate's resolution — do not re-walk; if the resolver is definition-shaped, extract the shared "position → qualified target name + origin span" helper — this is the #20-adjacent seam, keep it one function) → looks up `Analysis.docs` → renders plain text: paragraphs separated by blank lines; then one `deprecated: MSG`/`deprecated` line when applicable; then attention prose lines each on its own line prefixed `note: `. Span = the origin token's span. Hover on a declaration name uses the declaration's own doc. No doc → `None` (no empty hovers). Diagnostics: the merged-diagnostics path sets `deprecated: true` on findings whose code is `deprecated-call`. Completion: candidates resolving to deprecated functions set `deprecated: true`; `detail` = the fully-qualified name when it differs from the label (cross-namespace candidates), else `None`.

**Steps:**
- [ ] Failing tests: hover on call site (paragraphs render, blank-line separated); hover on declaration; hover on `use` path segment resolving to a documented fn; deprecated hover carries the callout; attention prose renders as `note:` lines; undocumented → None; `deprecated-call` diagnostic arrives with `deprecated: true`; completion candidate for a deprecated fn tagged; cross-namespace candidate carries qualified-name detail.
- [ ] Implement; pass; full gates.
- [ ] Commit: `feat(post-machine): pmc lsp hover with deprecation and attention callouts; tagged diagnostics and completions`

---

### Task 5: pma service — operand-hint detail (the #25 fold-in)

**Files:**
- Modify: `crates/post-machine/src/lsp/pma/complete.rs` (+ `pma/mod.rs` hover already `None` from Task 3)

Mnemonic candidates gain `detail` derived from the entry's `OperandKind`/`Flow`: `None` operand → no detail; `SymbolVec` → `wr <indices>` shape (use the real mnemonic); `RelI32|RelI8` + `Flow::Call` → `call <function>`; + `Flow::Jump|Branch` → `<mnemonic> <label>`. `.byte` → `.byte <0..=255>`; `.func` → `.func <name> [local]`. Directive/label/function candidates: no detail. `deprecated` stays false throughout (`.pma` has no attributes).

**Steps:**
- [ ] Failing tests: representative details per shape (`jm <label>`, `jmp.s <label>`, `call <function>`, `wr <indices>`, `nop` none, `.byte <0..=255>`); e2e completion carries `detail` on the wire.
- [ ] Implement; pass; full gates.
- [ ] Commit: `feat(post-machine): pma completion operand hints via candidate detail`

---

### Task 6: self-documenting stdlib + docs + checklists

**Files:**
- Modify: `crates/post-machine/src/stdlib/std.pmc` (docs for all 11 exported routines), `docs/lsp.md` (hover + tags + detail), `editors/vscode/README.md` + `editors/jetbrains/README.md` (hover checklist items, unticked)

Stdlib docs: read each routine's body and write accurate 1–3 line `?` docs (what it computes, head position contract, halt behavior); no `!` lines expected. The stdlib must still compile at `-O1` with byte-identical output (doc lines never reach codegen — pin by the existing golden/e2e suites staying untouched and green; `fmt_pma.rs`'s self-canonical sweep also stays green since `-S` output carries no docs).

**Steps:**
- [ ] Write the docs; run the FULL suite — goldens, `asm_acceptance`, `fmt_pma`, everything must pass UNCHANGED (any golden diff = docs leaked into codegen = bug).
- [ ] One new integration test: hover over a `std::` call in a scratch program returns the routine's first paragraph (through the staged LSP path).
- [ ] `docs/lsp.md`: hover section (plain text contract, deprecation callout, attention notes), tags, completion detail. Editor READMEs: hover walk item for `.pmc` (std:: call) — unticked.
- [ ] Full gates.
- [ ] Commit: `feat(post-machine): self-documenting stdlib; hover and tags documented`

---

## Self-Review (run after writing, fixed inline)

- Spec coverage: fmt ✔ (T1), lint channel ✔ (T2), hover + DiagnosticTag + CompletionItemTag + Candidate reshape ✔ (T3–T5 incl. #25), stdlib + docs/lsp.md + checklists ✔ (T6). Version/language docs were plan 1.
- Type consistency: `FnDoc { paragraphs, attention, deprecated }` and `Analysis.docs` consumed as plan 1 defines; `HoverContent { text, span }`, `Candidate.detail/deprecated`, `ServiceDiagnostic.deprecated` defined in T3 and consumed in T4–T5 by those names.
- Deliberate scope notes: pma hover stays `None` (spec); attention-prose presentation (`note:` prefix) is a T4 presentation decision consistent with the spec's "data layer stores verbatim, hover decides presentation".
