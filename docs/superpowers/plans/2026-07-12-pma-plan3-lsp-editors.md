# .pma parity Plan 3/3 — LSP language mux, PmaLanguageService, editor registration

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** One `pmt lsp` process serves both languages: the core framework gains multi-service routing (languageId-keyed, extension fallback, merged capabilities), a full-parity `PmaLanguageService` lands in `crates/post-machine/src/lsp/`, and both editor shells register `.pma` with a drift-guarded TextMate grammar.

**Architecture:** `server::run` widens to a service slice; a per-URI binding (recorded at `didOpen` from the client's own `languageId`) routes every later request. Capabilities merge mechanically: trigger chars + watched globs union, semantic-token legends concatenate with per-service index/bit remapping. The `PmaLanguageService` mirrors `PmcLanguageService`'s staging (total CST → lower/assemble fatal → lint) over the plan-1/2 machinery with `pm1_syntax()`.

**Tech Stack:** Rust edition 2024, zero new deps; TypeScript (VS Code shell), Kotlin/Gradle (JetBrains shell) — toolchains stay under `editors/` only. Design authority: `docs/superpowers/specs/2026-07-12-pma-parity-design.md` (LSP, Editors sections). Prerequisites: plans 1–2 merged.

## Global Constraints

- **Zero new Rust dependencies.** Gates at every commit: `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`. Editor sub-builds gate with their own `npm run compile` / `gradlew build` but are never wired into the repo-root toolchain.
- **Core carries zero PM-1/.pmc knowledge** — mux tests use two crate-private fake services with distinct fake language ids/legends.
- **No protocol-visible change for a single-service server** beyond the additive `extensions()` capability data — the `.pmc`-only editor shells from the LSP round must keep working unmodified until this plan's editor tasks update them.
- **Diagnostics severity mapping** stays presentation-side (`ServiceDiagnostic`), sources `"pmt"` (fatals) / `"pmt lint"` (findings) — identical to `.pmc`.
- Published docs ref-free; editors' READMEs are manual-checklist style.
- Conventional commits (`feat(core):`, `feat(post-machine):`, `feat(editors):`, `docs(editors):`). **No AI/Claude attribution footers.** Do NOT merge or push.

## File Structure

- `crates/core/src/lsp/mod.rs` — `LanguageService::extensions()` (Task 1).
- `crates/core/src/lsp/types.rs` — `languageId` on the didOpen params (Task 1).
- `crates/core/src/lsp/server.rs` — multi-service `run`, routing, capability merge (Task 2).
- `crates/post-machine/src/lsp/pma.rs` — `PmaLanguageService` (Tasks 3–4); `lsp/mod.rs` declares it.
- `crates/post-machine/src/cli/lsp.rs` — construct both services (Task 5).
- `docs/lsp.md` — second language + mux (Task 5).
- `editors/grammars/pma.tmLanguage.json` + `crates/post-machine/tests/editor_grammar.rs` — grammar + drift guard (Task 6).
- `editors/vscode/*`, `editors/jetbrains/*` — registrations + checklists (Task 7).

---

### Task 1: `extensions()` on the trait + `languageId` on didOpen

**Files:**
- Modify: `crates/core/src/lsp/mod.rs` (trait + FakeService)
- Modify: `crates/core/src/lsp/types.rs` (didOpen params struct)
- Modify: `crates/post-machine/src/lsp/mod.rs` (`PmcLanguageService` impl)

**Interfaces (Produces):**

```rust
// on trait LanguageService (after language_id):
/// File extensions (with dot) this service claims — the mux's fallback
/// when a client sends an unexpected languageId.
fn extensions(&self) -> &'static [&'static str];
```

`PmcLanguageService` returns `&[".pmc"]`; `FakeService` returns a fake (`&[".fake"]`). In `types.rs`, the `textDocument/didOpen` params' document item gains `language_id: String` (serde rename `languageId`) — check the existing struct first; the LSP client always sends it, the field was simply not consumed before. The server loop threads it to the (still single-service) open path but ignores it until Task 2.

**Steps:**
- [ ] Failing test: FakeService `extensions()` returns the fake list; didOpen deserialization test in `types.rs` includes `"languageId": "fake"` and the field parses.
- [ ] Implement (trait default NOT provided — force both impls to state their extensions); pass; full gates.
- [ ] Commit: `feat(core): LanguageService::extensions + didOpen languageId capture`

---

### Task 2: Multi-service `run` — routing + capability merge

**Files:**
- Modify: `crates/core/src/lsp/server.rs`
- Modify: `crates/post-machine/src/cli/lsp.rs` (adapt the call: wrap the single service — full dual wiring is Task 5)

**Interfaces (Produces):**

```rust
pub fn run(
    reader: &mut dyn std::io::BufRead,
    writer: &mut dyn std::io::Write,
    services: &mut [&mut dyn LanguageService],
    identity: ServerIdentity,
) -> i32;
```

Mechanics (each an implementation note + test):
- `ServerState` gains `bindings: HashMap<String, usize>` (uri → service index).
- **Routing:** `didOpen` binds by `language_id` (`services.iter().position(|s| s.language_id() == lang)`); unknown languageId falls back to the URI's extension matched against each service's `extensions()`; still unmatched → service 0 + a stderr note (never a hard error — diagnostics from the wrong language beat silence). `didChange`/`didClose`/requests look up the binding; a request on an unbound URI answers the existing not-open behavior. `did_close` removes the binding.
- **Capability merge** in `build_initialize_result`: trigger characters = ordered dedup union; semantic-token legend = concatenated `token_types` with a per-service **type-index offset table**, and dedup-union `token_modifiers` with a per-service **modifier-bit remap** (modifiers are bit positions — service-local bit `i` maps to merged bit `map[i]`). Store both maps in `ServerState`; `handle_semantic_tokens` applies them to each `SemToken { token_type, modifiers }` before wire packing.
- **Watched globs:** union across services (dedup, stable order) in `send_register_capability`.
- `workspace/didChangeConfiguration` broadcasts to every service; `workspace/didChangeWatchedFiles` republish walks all open docs through their bound service.
- Single-service call sites (existing tests) become `&mut [&mut service]` — behavior must be bit-identical (legend maps are identity for one service).

**Steps:**
- [ ] Failing mux tests in `server.rs`'s existing fake-service harness, with a second fake service (`FakeService2`: language id `"fake2"`, extension `".f2"`, legend `(["kw","number"], ["deprecated"])` overlapping `FakeService`'s in one modifier): (1) two didOpens route did_update to the right service (assert via each fake's recorded calls); (2) languageId `"plaintext"` + uri `foo.f2` falls back by extension; (3) completion/definition/formatting requests follow the binding; (4) merged initialize capabilities: trigger union, legend concat, modifier dedup; (5) a `SemToken` from service 1 arrives on the wire with remapped type index and modifier bits; (6) config change reaches both; (7) close unbinds; (8) single-service wrap produces byte-identical initialize result vs the pre-change expectation (update the stored expectation only if the serialization ORDER changes, not content).
- [ ] Implement; pass; adapt `cli/lsp.rs` minimally (`&mut [&mut service]`); full gates.
- [ ] Commit: `feat(core): lsp multi-service routing with merged capabilities`

---

### Task 3: `PmaLanguageService` — documents, diagnostics, formatting, code actions

**Files:**
- Create: `crates/post-machine/src/lsp/pma.rs`
- Modify: `crates/post-machine/src/lsp/mod.rs` (`mod pma;` + `pub(crate) use pma::PmaLanguageService;`)

**Interfaces (Produces):**

```rust
pub(crate) struct PmaLanguageService {
    docs: HashMap<String, PmaDocState>,
    ide_allow: Option<Result<Vec<String>, String>>,
    config_cache: HashMap<PathBuf, (SystemTime, Result<Vec<String>, String>)>,
}
// PmaLanguageService::new() -> Self

struct PmaDocState {
    text: String,
    cst: AsmCst,                          // total — always present
    functions: Option<Vec<SourceFunction>>, // lower success
    fatal: Option<AsmError>,              // lower/assemble failure
    lint: Option<Vec<Diagnostic>>,        // on full success only
    config_errors: Vec<String>,
}
```

`LanguageService` impl:
- `language_id` → `"pma"`; `extensions` → `&[".pma"]`; `trigger_characters` → `&['@', '.']`; `watched_globs` → `&["**/pmt.json"]`; `token_legend` → `(["function", "variable", "number"], ["declaration", "defaultLibrary"])` (labels ride "variable" with "declaration" on definitions).
- `did_update`: resolve config allow exactly like `PmcLanguageService` (factor the shared `project_allow`/`union_into`/`ide_allow` handling into `lsp/mod.rs`-level helpers reused by both services rather than copying — the config cache struct moves to a small shared `struct ConfigResolver` owned by each service). Then: `parse_asm_cst` (always) → `lower(&cst, &pm1_syntax())` → on success `assemble(...)` for the full fatal gate → on full success `mtc_core::asm::lint` rules via the shared context (call `mtc_core::asm::lint::lint(&syntax, text, &effective_allow)` — one call gives the gate AND findings; on `Err` store as fatal). Merged diagnostics mirror `merged_diagnostics`: config warnings first; a fatal short-circuits as one Error (`source: "pmt"`, `code: kind.code()`); otherwise lint findings (`source: "pmt lint"`), span-sorted. NOTE `.pma` has no compile-warning channel — the fatal/lint split is the whole story.
- `format` → `mtc_core::asm::format_asm(&state.text).ok()` (structural gate → `None`).
- `code_actions(uri, span)` → lint findings intersecting the span whose `fix` is `Some`, as `Action { title: fix.description, preferred: MachineApplicable, edits }` — port the `.pmc` service's span-intersection + edit conversion helper (reuse it if it's already a free fn; otherwise lift it to `lsp/mod.rs`).
- `did_close` / `did_change_config`: mirror `.pmc`'s (IDE allow parse; drop doc state).
- `completion`/`definition`/`document_symbols`/`semantic_tokens`: stubbed `None`/empty in THIS task (Task 4 fills them) — the service must be fully wired and useful for diagnostics+format first.

**Steps:**
- [ ] Failing unit tests (house style, in `pma.rs`): (1) clean program → no diagnostics; (2) unknown mnemonic → one Error diag with code `unknown-mnemonic`, span on the word; (3) listing line → `raw-line` Error; (4) unused label → one `pmt lint` Warning with the fix surfacing via `code_actions` at that span; (5) `--allow`-equivalent IDE config suppresses it (`did_change_config` with `{"lint":{"allow":["unused-label"]}}` — match the `.pmc` settings shape); (6) `format` grids a scrambled doc, `None` on a listing doc; (7) invalid `pmt.json` → `invalid-config` warning first.
- [ ] Implement (factoring the shared config-resolution helpers out of `PmcLanguageService` — its tests must stay green untouched); pass; full gates.
- [ ] Commit: `feat(post-machine): PmaLanguageService — diagnostics, formatting, lint quickfixes`

---

### Task 4: `PmaLanguageService` — completion, definition, symbols, semantic tokens

**Files:**
- Modify: `crates/post-machine/src/lsp/pma.rs`

**Feature specs (all single-document, all reading `PmaDocState`):**
- `completion(uri, pos)`: classify the cursor's line context from the CST + line text before `pos`: (a) instruction-word position (start of the word region — nothing or a partial word after labels) → all `pm1_syntax()` mnemonics as `Candidate`s (kind: keyword-ish; detail = operand hint, e.g. `jm <label>`) plus `.byte`/`.func` directives; (b) after `@` → exported+local `.func` names in the doc; (c) jump/branch operand position (word resolved to an entry with `RelI8|RelI32` + `Flow::Jump|Branch`) → labels of the enclosing function; (d) `call` operand → `.func` names. Enclosing function = the last `Func` item before the line.
- `definition(uri, pos)`: token under cursor via CST spans — a label reference in an operand → that label's `LabelCst` span in the same function (`DefTarget { uri: same, span, origin: Some(operand span) }`); a `call name`/`jmp @name` operand → the matching `FuncCst.name_span`.
- `document_symbols`: one `SymbolNode` per `FuncCst` (kind Function, span to the last item before the next Func), children = that function's labels (kind: reuse the closest available `SymbolNodeKind` — if only `Namespace|Function` exist, labels are Function children; do NOT widen the core enum unless a variant is truly missing, and if widening, add a generic `Label` variant to core with its LSP kind mapping in `server.rs`).
- `semantic_tokens`: walk the CST — `FuncCst.name_span` → `function` + `declaration`; call/`@` operand names that match a doc-local `.func` → `function`; label definitions → `variable` + `declaration`; label references in operands → `variable`; `Number` operand tokens → `number`. (Mnemonics stay TextMate's job.)

**Steps:**
- [ ] Failing tests per feature (follow the `.pmc` service's test style in `complete.rs`/`navigate.rs`/`tokens.rs` — if `pma.rs` grows past ~600 lines, split the same way: `pma/complete.rs`, `pma/navigate.rs`, `pma/tokens.rs`): mnemonic list at line start incl. after a label; `@` completion lists functions; branch operand lists only enclosing-function labels; definition on `jmp L1` operand lands on `L1:`; definition on `call helper` lands on `.func helper`; symbols tree function→labels; token walk produces the expected `(span, type, modifiers)` triples on the doc-example program; broken doc (unknown mnemonic) still answers symbols/completions (total CST!).
- [ ] Implement; pass; full gates.
- [ ] Commit: `feat(post-machine): pma lsp features — completion, definition, symbols, semantic tokens`

---

### Task 5: Dual-service `pmt lsp` + `docs/lsp.md`

**Files:**
- Modify: `crates/post-machine/src/cli/lsp.rs`
- Modify: `docs/lsp.md`

**Change (`cli/lsp.rs`):**

```rust
let mut pmc = crate::lsp::PmcLanguageService::new();
let mut pma = crate::lsp::PmaLanguageService::new();
let code = mtc_core::lsp::server::run(
    &mut stdin,
    &mut stdout,
    &mut [&mut pmc, &mut pma],
    mtc_core::lsp::server::ServerIdentity { name: "pmt lsp", version: env!("CARGO_PKG_VERSION") },
);
```

`docs/lsp.md`: a "Languages" section — the server serves `pmc` and `pma`; routing by the client's languageId with extension fallback; the `.pma` feature table (diagnostics incl. `raw-line`, formatting to the canonical grid, lint quickfixes, completion, definition, symbols, semantic tokens); the shared `pmt.json` allow union; ref-free prose.

**Steps:**
- [ ] Failing end-to-end test: extend the LSP integration coverage (the fake-transport harness used for the `.pmc` service, if present in `crates/post-machine/tests/`; otherwise add `tests/lsp_pma.rs` driving `server::run` over in-memory pipes with both real services): initialize (merged legend visible), didOpen a `.pma` doc with an unused label → published `pmt lint` diagnostic; didOpen a `.pmc` doc → routed correctly in the same session; formatting request on the `.pma` doc returns grid text.
- [ ] Implement wiring + docs; pass; full gates.
- [ ] Commit: `feat(cli): pmt lsp serves pmc and pma through one server`

---

### Task 6: `.pma` TextMate grammar + drift guard

**Files:**
- Create: `editors/grammars/pma.tmLanguage.json` (single source)
- Modify: `crates/post-machine/tests/editor_grammar.rs`

**Grammar scopes** (`scopeName: "source.pma"`): `comment.line.semicolon` (`;.*$`), `entity.name.function` on `.func` names, `keyword.control.directive` on `.func`/`.byte`, `keyword.other.mnemonic` on the 17 PM-1 mnemonics (word-bounded, incl. the `.s` forms — regex alternation generated in mnemonic-length order so `jm` never shadows `jm.s`), `entity.name.label` on `^[ \t]*[A-Za-z_][A-Za-z0-9_]*:`, `constant.numeric` on `-?\d+`, `variable.other.symbol` on `@[A-Za-z_][\w.:]*`.

**Drift guard:** a second test fn in `editor_grammar.rs` mirroring the `.pmc` one: parse `editors/grammars/pma.tmLanguage.json`, assert `scopeName == "source.pma"`, and assert the raw text contains **every mnemonic from `mtc_post_machine::asm::pm1_syntax()`** (iterate `entries`) plus `.func`, `.byte`, `local` — so a future mnemonic addition fails the suite until the grammar catches up.

**Steps:**
- [ ] Write the drift-guard test first — fails (no file); write the grammar; passes.
- [ ] Sanity: open a `.pma` sample with the grammar in VS Code manually later (Task 7's checklist covers it); the automated bar is JSON-validity + coverage.
- [ ] Full gates.
- [ ] Commit: `feat(editors): shared pma TextMate grammar + pm1_syntax drift guard`

---

### Task 7: Editor registrations + manual checklists

**Files:**
- Modify: `editors/vscode/package.json`, `editors/vscode/src/extension.ts`, `editors/vscode/scripts/copy-grammar.js`, `editors/vscode/README.md`
- Create: `editors/vscode/language-configuration-pma.json`
- Modify: `editors/jetbrains/src/main/resources/META-INF/plugin.xml`, `editors/jetbrains/build.gradle.kts` (grammar copy), `editors/jetbrains/README.md`
- Create: `editors/jetbrains/src/main/kotlin/ru/mellonis/pmc/PmaFileType.kt`, `editors/jetbrains/src/main/resources/textmate/pma/package.json`

**VS Code:**
- `package.json`: `activationEvents` += `"onLanguage:pma"`; `contributes.languages` += `{ "id": "pma", "extensions": [".pma"], "aliases": ["PMA"], "configuration": "./language-configuration-pma.json" }`; `contributes.grammars` += `{ "language": "pma", "scopeName": "source.pma", "path": "./syntaxes/pma.tmLanguage.json" }`.
- `language-configuration-pma.json`: `{ "comments": { "lineComment": ";" } }`.
- `copy-grammar.js`: copy both grammars from `editors/grammars/`.
- `extension.ts`: `documentSelector: [{ language: 'pmc' }, { language: 'pma' }]`; task-provider gate `doc.languageId !== 'pmc'` widens to accept `pma` **for the `lint` and `fmt-check` task types only** (the `compile` task stays `.pmc` — `.pma` compiles via `pmt asm`, out of the task provider's v1 scope; note this in the README).
- `npm run compile` must pass; bump the extension patch version.

**JetBrains:**
- `PmaFileType.kt`: clone of `PmcFileType` (name "PMA", extension `pma`, its own description/icon reuse).
- `plugin.xml`: `<fileType name="PMA" … extensions="pma"/>`; `editorHighlighterProvider filetype="PMA"`; `<fileTypeMapping fileType="PMA" serverId="pmtLsp" languageId="pma"/>` (same server — the mux does the rest).
- TextMate: `textmate/pma/package.json` bundle manifest + `build.gradle.kts` copies `pma.tmLanguage.json` into `textmate/pma/`; the bundle provider returns both bundles (adjust `PmcTextMateBundleProvider` to a list if the LSP4IJ/TextMate API took a single bundle — check its current return type and extend minimally).
- `gradlew build` must pass.

**Both READMEs:** append a `.pma` manual-checklist section: open a `.pma` file → highlighting; typo mnemonic → squiggle with `unknown-mnemonic`; unused label → lint warning + quickfix removes it; go-to-definition on a jump target; outline shows functions/labels; format-document grids the file; paste a `--listing` snippet → `raw-line` error; `.pmc` files still fully functional in the same session.

**Steps:**
- [ ] VS Code changes; `npm run compile` green.
- [ ] JetBrains changes; `./gradlew build` green (from `editors/jetbrains/`).
- [ ] READMEs + checklists (the checklist walk itself is the user's manual acceptance, as in the LSP round — record placeholders unticked).
- [ ] Full repo gates (Rust untouched here but run anyway).
- [ ] Commit: `feat(editors): register .pma in both shells — selectors, file types, checklists`

---

## Self-Review (run after writing, fixed inline)

- Spec coverage: `extensions()` + languageId ✔ (T1), mux routing/merge/broadcast ✔ (T2), full-parity service — diagnostics/format/actions ✔ (T3) + completion/definition/symbols/tokens ✔ (T4), dual wiring + docs/lsp.md ✔ (T5), grammar + drift guard ✔ (T6), both editors + checklists ✔ (T7).
- Type consistency: consumes plan-1/2 names exactly (`parse_asm_cst`, `lower`, `AsmError.kind.code()`, `format_asm`, `mtc_core::asm::lint::lint`, `pm1_syntax().entries`); core mux consumes `LanguageService` exactly as amended in T1.
- Deliberate scope note: cross-file definition, hover, DAP stay out per the spec's Out-of-scope list.
