# `pmt lsp` Plan 2/3 — the `.pmc` language service, config, and the `pmt lsp` subcommand

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** The `.pmc` implementation of core's `LanguageService`: staged analysis with honest degradation, three-channel diagnostics, completions, go-to-definition (incl. the materialized stdlib), document symbols, lint quickfixes, semantic tokens, formatting via `format()`, `pmt.json` project config shared with the CLI, fatal error codes with CLI parity, and the `pmt lsp` subcommand.

**Architecture:** Compiler extensions first (fatal codes, additive reference spans, the resolution table, a staged-analysis entry, the stdlib roster), then the service (`crates/post-machine/src/lsp/`) implementing the trait from plan 1 verbatim, then the CLI wiring. Single-source-of-truth rules: call resolution lives in `flatten` (the resolution table), the command vocabulary is `parser::RESERVED`, the std roster derives from the embedded `SOURCE`, formatting is `fmt::format` — the LSP layer re-implements none of them.

**Tech Stack:** Rust edition 2024; no new deps. **Prerequisite: plan 1 (`2026-07-10-lsp-plan1-core-framework.md`) is fully landed** — this plan consumes `mtc_core::lsp::{LanguageService, ServiceDiagnostic, ServiceSeverity, Candidate, CandidateKind, DefTarget, Action, SymbolNode, SymbolNodeKind, SemToken, server::run, server::ServerIdentity}` exactly as frozen there. Design authority: `docs/superpowers/specs/2026-07-07-pmt-lsp-design.md`.

## Global Constraints

Every task's requirements implicitly include these. Copy the binding ones into each task's reviewer prompt.

- **Zero new dependencies.** Gates at every commit: `cargo test --workspace` green, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`. Commit only on a clean tree.
- **Behavior freeze outside the sanctioned changes:** `pmt compile`/`lint`/`fmt` outputs stay byte-identical EXCEPT the two deliberate changes — (a) the bracketed fatal-code suffix (Task 1), (b) `undefined-label`'s span becoming character-precise (Task 2). `-O0` bit-identity and the golden suite stay green untouched. `PMC_LANG_VERSION` stays `0.2` (spans and side tables are metadata, not grammar). `IR_VERSION` stays 3.
- **No stale positions:** every feature answers from the document's current text or returns null/empty. The single sanctioned staleness: completion *names* from the last-good analysis.
- **Fail-fast fatals kept:** one honest error, never a cascade; no parser error recovery.
- **Thin renderer:** only `cli/` prints; `cli/lsp.rs` is the only place real stdio is handed to the server loop.
- **Fatal codes are stable identifiers once shipped** — the Task 1 table is the naming pass; changing a name later is a breaking documentation change.
- **Published docs are forge-agnostic** (no issue/PR numbers, no host URLs): `docs/lsp.md`, `docs/cli.md`, `docs/lint.md`, `README.md`, code comments.
- **Conventional commits**, scopes `feat(post-machine):` / `feat(cli):` / `test(post-machine):` / `docs(lsp):`. **No AI/Claude attribution footers.**
- Do **NOT** merge or push; the branch is left for the user's review.

## File Structure

- `crates/post-machine/src/compiler.rs` — `CompileErrorKind::code()` (Task 1); `Resolution` + resolution table in `flatten` (Task 3); `analyze_staged` (Task 4).
- `crates/post-machine/src/parser.rs` + `src/cst.rs` — additive reference/extent spans (Task 2).
- `crates/post-machine/src/ir.rs` — `undefined-label` span upgrade (Task 2).
- `crates/post-machine/src/lint/mod.rs` — `validate_allow` + `run_rules` split (Task 4).
- `crates/post-machine/src/stdlib/mod.rs` — `roster()`, `materialized_std_uri()` (Task 5).
- `crates/post-machine/src/config.rs` (new) — `pmt.json` discovery/parse (Task 6).
- `crates/post-machine/src/cli/lint.rs` — `--no-config` + per-file config (Task 6).
- `crates/post-machine/src/lsp/mod.rs` (new) — `PmcLanguageService`, `DocState`, diagnostics, config plumbing (Task 7); document symbols (Task 8).
- `crates/post-machine/src/lsp/navigate.rs` (new) — go-to-definition (Task 9).
- `crates/post-machine/src/lsp/complete.rs` (new) — completions (Task 10).
- `crates/post-machine/src/lsp/tokens.rs` (new) — semantic tokens (Task 12).
- `crates/post-machine/src/cli/lsp.rs` (new) + `cli/mod.rs` + `completions/registry.rs` (Task 13).
- `docs/lsp.md` (new), `docs/cli.md`, `docs/lint.md`, `README.md` (Tasks 1, 6, 13).

The service module is `mod lsp;` in `lib.rs` — **not** `pub` (no external library surface; the CLI reaches it crate-internally). All service tests are inline `#[cfg(test)]` (the service types are crate-private, unreachable from `tests/`).

---

### Task 1: Fatal error codes + CLI parity

**Files:**
- Modify: `crates/post-machine/src/compiler.rs`, `crates/post-machine/src/cli/build.rs` / `cli/lint.rs` / `cli/fmt.rs` (whichever render `error: {kind}`), `docs/cli.md`.
- Tests asserting exact rendered error text across the workspace update in the same change.

**Interfaces (Produces):**

```rust
impl CompileErrorKind {
    /// Stable kebab-case code, one per variant (docs/cli.md (compile errors)).
    pub fn code(&self) -> &'static str { … }
}
```

The naming pass (frozen — all 19 variants):

| variant | code |
|---|---|
| `Lex(_)` | `lex-error` |
| `Expected { .. }` | `unexpected-token` |
| `ReservedName { .. }` | `reserved-name` |
| `UnknownCommand(_)` | `unknown-command` |
| `BuiltinCalled(_)` | `builtin-called` |
| `EmptyBuiltinParens { .. }` | `empty-builtin-parens` |
| `DuplicateName { .. }` | `duplicate-name` |
| `DuplicateLabel(_)` | `duplicate-label` |
| `UndefinedLabel(_)` | `undefined-label` |
| `GotoReturn` | `goto-return` |
| `GroupPosition(_)` | `group-position` |
| `DanglingLabel(_)` | `dangling-label` |
| `Internal(_)` | `internal-error` |
| `NestedExport` | `nested-export` |
| `DuplicateBinding(_)` | `duplicate-binding` |
| `KeywordNeedsName(_)` | `keyword-needs-name` |
| `KeywordInBody(_)` | `keyword-in-body` |
| `SingleColonInPath` | `single-colon-in-path` |
| `TopLevelStatement(_)` | `top-level-statement` |

(`lex-error`/`internal-error`/`unexpected-token` deviate from bare variant names deliberately — `lex`, `internal`, and `expected` are too generic as user-visible identifiers; everything else is the variant name kebab-cased.)

Rendering: `CompileError`'s `Display` becomes `line {l}:{c}: {kind} [{code}]`. The CLI renderers that format the *kind* directly (the `{file}:{line}:{col}: error: {kind}` per-file fatal lines in `cli/lint.rs`, `cli/fmt.rs`, and the compile path) append ` [{code}]` the same way — grep `cli/` for `error: {}` renderings and sweep them all. The LSP (Task 7) uses `kind.to_string()` as the message and puts the code in the diagnostic's `code` field — so the *kind's* own `Display` must NOT carry the suffix (no duplication in editors).

**Steps (TDD):**
- [ ] Failing tests in `compiler.rs`: (1) `code()` values are pairwise distinct (collect all 19 via representative constructed kinds into a `HashSet`, assert len — exhaustiveness is free, the `match` in `code()` is over the enum); (2) `CompileError { span, kind: DuplicateLabel(5) }.to_string() == "line 3:7: duplicate label `5` [duplicate-label]"`.
- [ ] Implement `code()` + the `Display` suffix + the CLI render-site sweep.
- [ ] Run `cargo test --workspace`; update every test that asserts exact rendered error text (compile/cli/fmt/lint suites) to expect the suffix. This sweep is mechanical — the *set* of failing tests is the blast radius the spec predicted.
- [ ] `docs/cli.md`: add a **compile errors** table (code | meaning, one row per code, prose descriptions) in the `pmt compile` section, plus a note that every fatal rendering carries the bracketed code suffix. Forge-agnostic wording.
- [ ] Full gates green. Commit: `feat(post-machine): stable kebab-case codes on compile fatals, bracketed in every CLI rendering`

---

### Task 2: Additive reference + extent spans (parser, CST, ir)

**Files:**
- Modify: `crates/post-machine/src/parser.rs`, `crates/post-machine/src/cst.rs`, `crates/post-machine/src/ir.rs`.

**Interfaces (Produces)** — all additive fields, carried verbatim through the CST's item embedding:

```rust
// parser.rs — Item gains reference spans:
pub enum Item {
    Builtin { which: Builtin, succ: Successor, succ_span: Option<Span>,
              succ_label_span: Option<Span>,   // NEW: Some iff succ is Successor::Label — the number token
              line: u32 },
    Debugger { line: u32 },
    Call    { name: String, name_span: Span, succ: Successor, succ_span: Option<Span>,
              succ_label_span: Option<Span>,   // NEW: same rule
              line: u32 },
    Check   { marked: CheckArm, blank: CheckArm, span: Span,
              marked_span: Span, blank_span: Span,   // NEW: each arm's token (number or `!`)
              line: u32 },
    Halt    { line: u32 },
    Goto    { label: u32,
              label_span: Span,                // NEW: the target number token
              line: u32 },
}

// cst.rs — extent spans for hit-testing and document symbols:
pub struct FunctionCst  { …existing…, pub span: Span }   // NEW: header first token → closing `}` end
pub struct NamespaceCst { …existing…, pub span: Span }   // NEW: `namespace` keyword → closing `}` end
```

`ir::lower`'s `UndefinedLabel` error uses the new reference spans instead of `Span::point(line, 1)` — the `resolve` closure takes the reference span (goto target / check arm / successor) and puts it on the `CompileError`. This makes `goto 99` squiggle the `99`, not column 1.

**Blast radius (mechanical, not risky):** every struct-literal construction of these variants/structs — the parser's build sites and every hand-constructed AST/CST in tests — gains the new fields. Pattern matches using `..` are untouched. `lower_cst` copies the new fields through verbatim (it clones `Item`s wholesale already). The ir test pinning `Span::point(line, 1)` updates to the precise span. fmt only reads via patterns — unaffected; the fmt corpus harness (idempotence/behaviour/comment-fidelity) is the objective guard that nothing regressed.

**Steps (TDD):**
- [ ] Failing parser tests: for the source `f() { 1: right(2); check(1, !); goto 1; left, mark(3); }` assert, with exact `Span::new(...)` values: `Goto.label_span` covers the `1`; `Check.marked_span` covers the `1`, `blank_span` covers the `!`; `succ_label_span` on the `right(2)` builtin covers the `2` (inside the parens, number only) and is `None` on a bare `right;`; `succ_label_span` on a call `@g(7);` covers the `7`. And: `FunctionCst.span` / `NamespaceCst.span` cover header-through-`}` for a two-line function and a namespace block.
- [ ] Failing ir test: `goto 9` in a one-function program errors `undefined-label` with the span of the `9` (not col 1); same for a `left(7)` successor and a `check(7, !)` arm.
- [ ] Implement; sweep test construction sites; full workspace green (fmt corpus harness included).
- [ ] Full gates. Commit: `feat(post-machine): character-precise spans on label references and CST node extents`

---

### Task 3: The resolution table (flatten extension)

**Files:**
- Modify: `crates/post-machine/src/compiler.rs`.

**Interfaces (Produces):**

```rust
/// How flatten resolved one call site (docs/lsp.md (navigation)).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Resolution {
    /// A function this module defines (incl. nested and qualified-internal).
    Local { def_name_span: Span },
    /// A bare name bound by a `use` import.
    ImportBinding { use_span: Span, full_path: String },
    /// A `@ns::name()` call whose target this module does NOT define.
    QualifiedExternal { full_path: String },
    /// A bare undeclared external.
    Unresolved,
}
```

- `flatten`'s return grows into `struct Flattened { program: Program, scopes: ScopeSummary, warnings: Vec<Diagnostic>, resolutions: Vec<(Span, Resolution)> }` (replaces the 3-tuple; `analyze`/`compile` destructure it).
- `AnalysisOutput` gains `pub resolutions: Vec<(Span, Resolution)>`.
- Recording, inside `resolve()` (compiler.rs ~402–448), one entry per `Item::Call`, keyed by the call's `name_span`:
  - qualified (`name.contains("::")`): check the written path against the module's defs-by-full-name (the union of `ctx.defs` values — qualified calls can only target namespace-level functions, never `.`-nested ones); a hit records `Local`, a miss records `QualifiedExternal { full_path: name.clone() }`. The compiler already proves in-module qualified calls internal (its reachability pass builds edges from them) — the table just says so.
  - nested-map hit → `Local` (the mangled `{prefix}.{name}`); defs hit → `Local`; bindings hit → `ImportBinding { import_index }`; total miss → `Unresolved`.
  - Implementation note: record an internal `RawResolution` (`Local{mangled: String}` / `ImportBinding{index: usize}` / …) during the walk, then one post-pass at the end of `flatten` converts it — `mangled → def_name_span` via a map built from the flattened `program.functions` (names are unique post-mangle), `index → (imports[i].span, imports[i].full_path())`. This avoids ordering problems (a call can resolve to a nested fn that hasn't been emitted yet).

**Steps (TDD):**
- [ ] Failing tests in `compiler.rs` (drive via `analyze`, assert `resolutions`): a fixture exercising every arm —
  ```pmc
  use ext;
  use std::goToEnd as ge;
  namespace ns { export inner() { right; } }
  export main() {
      helper() { left; }
      @helper();        // Local (nested)
      @ns::inner();     // Local (qualified-internal → def_name_span of inner)
      @inner();         // Unresolved (ns member is not visible as a bare name here)
      @ext();           // ImportBinding (full_path "ext")
      @ge();            // ImportBinding (full_path "std::goToEnd")
      @other::thing();  // QualifiedExternal
      @mystery();       // Unresolved (+ existing undeclared-external warning)
  }
  ```
  Assert each entry's span (the call `name_span`) and the exact `Resolution` (with `def_name_span` matching the definition's `name_span`, `use_span` matching the import's `span`). Adjust the fixture if any line trips an unrelated warning that obscures the assert — the point is one entry per call, correctly classified.
- [ ] Assert compile output unchanged: an existing compile/golden test run confirms the table is a pure side channel.
- [ ] Implement (RawResolution + post-pass); green; full gates.
- [ ] Commit: `feat(post-machine): flatten records a per-call-site resolution table`

---

### Task 4: Staged analysis + lint split

**Files:**
- Modify: `crates/post-machine/src/compiler.rs`, `crates/post-machine/src/lint/mod.rs`.

**Interfaces (Produces):**

```rust
// compiler.rs — the LSP's pipeline entry (docs/lsp.md (staged analysis)):
pub(crate) struct Analysis {
    pub ast: Program,                          // flattened
    pub scopes: ScopeSummary,
    pub warnings: Vec<Diagnostic>,             // ir + visibility, same order as analyze()
    pub resolutions: Vec<(Span, Resolution)>,
}
pub(crate) struct StagedAnalysis {
    pub tokens: Option<Vec<Token>>,            // WithComments — None only if lexing failed
    pub cst: Option<Cst>,                      // None if lexing or parsing failed
    pub analysis: Option<Analysis>,            // None if any stage failed
    pub fatal: Option<CompileError>,           // the first (only) fatal
}
/// Runs lex→parse_cst→lower_cst→binding check→flatten→ir::lower, retaining
/// each stage's outcome. flatten is infallible; the post-parse fatals are
/// DuplicateBinding (binding check) and UndefinedLabel (lower) — the pipeline
/// runs through ir::lower, never stops at flatten.
pub(crate) fn analyze_staged(source: &str) -> StagedAnalysis;
```

`analyze()` itself stays as-is (WithoutComments tokens — lint's CLI path and `compile()` are untouched); `analyze_staged` is the LSP's entry. The WithComments significant-token stream filtered of `TokenKind::Comment` is byte-identical to the WithoutComments stream (the fmt work guaranteed this) — the service exploits that for lint.

```rust
// lint/mod.rs — split so the service can lint an existing analysis:
pub(crate) fn validate_allow(codes: &[String]) -> Result<(), LintError>;   // the RULES check, extracted
pub(crate) fn run_rules(ctx: &LintContext, allow: &[String]) -> Vec<Diagnostic>;  // the rule loop + span sort, extracted
// pub fn lint(...) rewired over the two — behavior byte-identical.
```

**Steps (TDD):**
- [ ] Failing tests for `analyze_staged` — one per degradation tier: (1) clean source → all four stages `Some`, `fatal: None`, warnings/resolutions equal to `analyze()`'s on the same source (tokens differ only by `Comment` entries); (2) lex failure (unterminated block comment) → everything `None` + fatal `lex-error`; (3) parse failure (`f() { gibberish }`… use a real grammar error) → `tokens: Some`, `cst: None`, `analysis: None`, fatal set; (4) binding-check failure (a duplicate `use` binding) → `cst: Some`, `analysis: None`, fatal `duplicate-binding`; (5) lower failure (`goto 99`) → `cst: Some`, `analysis: None`, fatal `undefined-label` — proving the pipeline runs through `ir::lower`.
- [ ] Failing lint test: `lint()` output on the existing lint fixtures is byte-identical before/after the split (pin one report).
- [ ] Implement; green; full gates.
- [ ] Commit: `feat(post-machine): staged analysis entry for the lsp; lint split into validate_allow + run_rules`

---

### Task 5: Stdlib roster + materialized std

**Files:**
- Modify: `crates/post-machine/src/stdlib/mod.rs`.

**Interfaces (Produces):**

```rust
pub(crate) struct RosterEntry {
    pub full_path: String,    // "std::goToEnd"
    pub name_span: Span,      // the routine name token in std.pmc
    pub decl_line: u32,
}
/// Parses SOURCE once (OnceLock) into the exported-routine roster.
pub(crate) fn roster() -> &'static [RosterEntry];

/// The embedded std.pmc written once per toolchain version to
/// <cache>/pmt/<version>/std.pmc — $XDG_CACHE_HOME falling back to
/// ~/.cache on unix, %LOCALAPPDATA% on windows — returned as a file: URI.
/// Any IO failure degrades to None (docs/lsp.md (materialized stdlib)).
pub(crate) fn materialized_std_uri() -> Option<&'static str>;   // OnceLock<Option<String>>
```

Implementation notes: `roster()` = `lex(SOURCE)` → `parse_cst` → walk the `std` namespace block's `FunctionCst`s where `exported` — no hand parsing. Materializer: create dirs, write if the file is absent or its bytes differ from `SOURCE` (self-heals corruption); build the `file:` URI with a small local `path_to_file_uri` helper (absolute path, forward slashes, percent-encode everything outside RFC 3986 unreserved + `/`; on windows prefix `file:///C:/…`).

**Steps (TDD):**
- [ ] Failing tests: (1) **drift guard** — `roster()` names equal the *exported* symbol names on `object()` (`SymbolDef::Defined`), as sets; count is 11; (2) each `name_span` lands exactly on the routine's name text in `SOURCE` (slice the source line by the span and compare to the last path segment); (3) **ASCII guard** — for every entry, the source line holding `name_span` is pure ASCII (this is what makes the framework's char==UTF-16 fallback conversion exact for std targets — plan 1 Task 9's documented contract); (4) materializer round-trip using a temp `XDG_CACHE_HOME`/`LOCALAPPDATA` (set via a test-scoped env override — mark the test `#[serial]`-like by using a dedicated env var read at call time… simplest: factor the cache-root lookup into `fn cache_root() -> Option<PathBuf>` and test the write+URI logic through an inner `materialize_into(root: &Path) -> Option<String>` that the public fn wraps — no env mutation in tests); file exists + bytes == `SOURCE` + URI starts with `file://`; (5) corrupted existing file gets rewritten.
- [ ] Implement; green; full gates.
- [ ] Commit: `feat(post-machine): stdlib roster + materialized std.pmc cache for lsp navigation`

---

### Task 6: `pmt.json` — config module + CLI integration

**Files:**
- Create: `crates/post-machine/src/config.rs`; wire `mod config;` in `lib.rs`.
- Modify: `crates/post-machine/src/cli/lint.rs`, `src/completions/registry.rs`, `docs/cli.md`, `docs/lint.md`.

**Interfaces (Produces):**

```rust
//! Project configuration: pmt.json, the toolchain's first (deliberately
//! tiny) project file (docs/lint.md (project file)).
pub(crate) struct ProjectConfig { pub allow: Vec<String> }

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ConfigError {
    Io { path: PathBuf, message: String },
    Parse { path: PathBuf, message: String },        // unparseable JSON
    UnknownKey { path: PathBuf, key: String },        // typo must not silently disable itself
    UnknownAllowCode { path: PathBuf, code: String },
}  // + Display ("pmt.json: unknown key `lints`" shape, path included) + std::error::Error

/// Nearest-ancestor walk from `start` (the source file's directory) to the
/// filesystem root; first pmt.json wins — never a cascade.
pub(crate) fn discover(start: &Path) -> Option<PathBuf>;
pub(crate) fn load(path: &Path) -> Result<ProjectConfig, ConfigError>;
```

Schema validation is a manual `serde_json::Value` walk (precise errors beat serde's): top level allows exactly `lint`; `lint` allows exactly `allow`; `allow` must be an array of strings; codes validated via `lint::validate_allow` (mapped to `UnknownAllowCode`). An empty JSON object `{}` is valid (empty allow).

CLI side (`cli/lint.rs`): a new `--no-config` boolean flag (ignores project files entirely — CI runs that want flag-only behavior). Per input file: `discover(file.parent())` → `load` → effective allow = **union** of `--allow` flags ∪ file allow (dedup). A `ConfigError` is a hard **per-file** error: stderr line `{pmt.json path}: error: {message}`, the file is skipped, the batch continues, exit code 1 (the per-file fatal model; the `--allow` flag's own unknown-code error stays whole-tool, unchanged).

**Steps (TDD):**
- [ ] Failing `config.rs` unit tests (tempdir-based): nearest-wins across nested dirs (`a/pmt.json` + `a/b/pmt.json`, source in `a/b/c/` → `a/b/pmt.json` wins, the ancestor is ignored, not merged); no file → None; unparseable JSON → `Parse`; `{"lints":{}}` → `UnknownKey("lints")`; `{"lint":{"allowed":[]}}` → `UnknownKey("allowed")`; `{"lint":{"allow":["no-such"]}}` → `UnknownAllowCode`; `{"lint":{"allow":["unused-label"]}}` → ok.
- [ ] Failing CLI tests (extend `tests/lint_programs.rs` / `cli_programs.rs`, tempdir fixtures): (1) a `pmt.json` allowing `unused-label` suppresses the finding that fires without it; (2) union with `--allow` (file allows one code, flag another, both suppressed); (3) `--no-config` ignores the file (finding fires again); (4) invalid `pmt.json` → stderr names the pmt.json path + message, exit 1, a second clean input file in the same run still lints.
- [ ] Registry: `lint_spec()` gains `FlagSpec::boolean("--no-config", "ignore pmt.json project files")`; the drift-guard suite (`tests/completions_registry.rs`) stays green (it probes the real parser with the new flag).
- [ ] Docs: `docs/lint.md` gains a **Project file: `pmt.json`** section (schema — `lint.allow` only; nearest-ancestor discovery, nearest-wins never cascade; union-across-sources semantics: file ∪ flags ∪ IDE settings; strict validation posture). `docs/cli.md` `pmt lint` section documents `--no-config` + the per-file config error posture.
- [ ] Full gates. Commit: `feat(post-machine): pmt.json project config — nearest-ancestor discovery, union allow-lists, --no-config`

---

### Task 7: Service skeleton — `PmcLanguageService`, staged state, three-channel diagnostics, config plumbing

**Files:**
- Create: `crates/post-machine/src/lsp/mod.rs`; wire `mod lsp;` in `lib.rs`.

**Interfaces (Produces):**

```rust
//! The .pmc language service: implements mtc_core::lsp::LanguageService
//! over the real compiler pipeline (docs/lsp.md).
pub(crate) struct PmcLanguageService {
    docs: HashMap<String, DocState>,
    /// IDE-settings allow-list: None = never configured; Ok = valid codes;
    /// Err = human-readable reason (surfaces as invalid-config).
    ide_allow: Option<Result<Vec<String>, String>>,
    /// pmt.json parse cache keyed by winner path; (mtime, outcome).
    config_cache: HashMap<PathBuf, (SystemTime, Result<Vec<String>, String>)>,
}
impl PmcLanguageService { pub(crate) fn new() -> Self; }

struct DocState {
    text: String,
    tokens: Option<Vec<Token>>,              // WithComments, current text
    cst: Option<Cst>,                         // current text
    analysis: Option<Analysis>,               // current text
    lint: Option<Vec<Diagnostic>>,            // findings + retained fixes
    fatal: Option<CompileError>,
    /// Names-only staleness exception: last-good scopes survive a failed
    /// re-analysis so completion candidates stay useful mid-edit.
    scopes_for_completion: Option<ScopeSummary>,
    /// invalid-config messages that applied to this analysis (0..=2 entries).
    config_errors: Vec<String>,
}
```

Trait constants: `language_id` `"pmc"`, `trigger_characters` `['@', ':']`, `token_legend` `(&["namespace", "function", "number"], &["declaration", "defaultLibrary"])`, `watched_globs` `&["**/pmt.json"]`.

`did_update(uri, text)`:
1. Resolve config: `uri_to_path(uri)` (a local helper: `file:` URIs → percent-decoded `PathBuf`, anything else — `untitled:` — → None). With a path: re-run `config::discover` from its parent **every analysis** (a few stats; a newly created nearer `pmt.json` must win), then consult `config_cache` by winner path — reuse the parsed outcome only when mtime is unchanged, else `config::load` and cache. No winner → no project source.
2. Effective allow = union of project-file allow (if Ok) ∪ IDE allow (if `Some(Ok)`); each Err source contributes one `invalid-config` entry instead.
3. `analyze_staged(text)`; on success build `LintContext { source, tokens: <WithComments filtered of Comment>, ast: &analysis.ast, scopes: &analysis.scopes }` and `run_rules(ctx, &effective_allow)`.
4. Store `DocState`; return the merged set (spec table):
   - each `invalid-config` entry → Warning, source `"pmt"`, code `Some("invalid-config")`, `Span::point(1, 1)`, message naming the source (the pmt.json path, or "IDE settings") + reason;
   - `fatal` → exactly one Error, source `"pmt"`, code `Some(kind.code())`, message `kind.to_string()`;
   - else: compile warnings (source `"pmt"`, their codes) + lint findings (source `"pmt lint"`, their codes), merged in span order.

`did_close(uri)`: drop the state (the framework publishes the empty set).
`did_change_config(settings)`: unwrap `settings["pmt"]` when that key exists (clients that forward whole sections), else use the value directly; read `lint.allow`; missing → `ide_allow = None` (unconfigured, not invalid); a non-array / non-string entries / unknown codes (via `validate_allow`) → `Some(Err(reason))`; valid → `Some(Ok(codes))`. **Other keys are ignored** — the IDE channel carries client-owned settings like the binary path; strictness belongs to `pmt.json` only. No republish here — the framework re-runs `did_update` for every open doc after this call (plan 1 Task 9).

**Steps (TDD)** (fixtures as string literals; drive the trait methods directly):
- [ ] Failing tests — diagnostics merge: (1) parse-failure source → exactly one Error with the right code, nothing else; (2) a clean-parse source with one compile warning (`use unused;`) + one lint finding (an unused label) → both, correct sources/severities/codes, span-ordered; (3) `goto 99` → single `undefined-label` Error at the char-precise span (Tasks 2+4 visibly paying off); (4) fix retention: the lint finding's `Fix` is reachable in `DocState.lint` (Task 11 consumes it).
- [ ] Failing tests — staleness exception: open a clean doc (analysis succeeds), then update to a parse-broken revision → `scopes_for_completion` still holds the last-good scopes while `cst`/`analysis` are None.
- [ ] Failing tests — config: (a) tempdir with `pmt.json` allowing the lint code + a `file:` URI into it → finding suppressed; (b) rewrite `pmt.json` with a *newer mtime* and a broken schema → next `did_update` yields the `invalid-config` warning at 1:1 AND the finding back (lint ran with remaining sources); (c) IDE settings `{"lint":{"allow":["unused-label"]}}` via `did_change_config` → suppressed on next update; wrapped `{"pmt":{"lint":…}}` works identically; (d) IDE settings with an unknown code → `invalid-config` naming IDE settings; (e) union: file allows code A, IDE allows code B → both suppressed; (f) `untitled:` URI → no project config, no error.
- [ ] Implement; green; full gates.
- [ ] Commit: `feat(post-machine): PmcLanguageService — staged doc state, three-channel diagnostics, project + IDE config`

---

### Task 8: Formatting + document symbols

**Files:**
- Modify: `crates/post-machine/src/lsp/mod.rs`.

**Interfaces:** the two CST-tier trait methods:

- `format(uri)`: `crate::fmt::format(&state.text)` — `Ok(t) → Some(t)`, `Err(_) → None` (quiet; the parse error is already a published diagnostic). Byte-identical to `pmt fmt` by construction — same function.
- `document_symbols(uri)`: `state.cst.as_ref()?` → walk: `TopKind::Namespace` → `SymbolNode { kind: Namespace, span: ns.span, selection_span: ns.name_span, children: <recurse into its items> }` (reopened blocks stay separate siblings — the CST already keeps them apart); `TopKind::Function` → `kind: Function`, children = its `BodyKind::Nested` functions recursively. `span` = the Task 2 extent span, `selection_span` = `name_span`. Labels are NOT emitted. Works while post-parse analysis fails (CST-tier).

**Steps (TDD):**
- [ ] Failing tests: (1) a fixture with `namespace a { f() {…} }`, a reopened `namespace a { g() {…} }`, and a top-level `main` with a nested `helper` → the exact expected tree (two separate `a` siblings; `helper` a child of `main`; no label nodes; ranges = extent spans, selections = name spans); (2) symbols still answered when the source has a post-parse fatal (`goto 99` — parse fine, lower fails); (3) `None` when parsing failed; (4) `format` on an unformatted-but-valid source equals `fmt::format` on the same input (assert equality with a direct call — the single-source contract); (5) `format` returns None on a parse error; (6) already-formatted source returns the identical text (the framework's empty-edit path).
- [ ] Implement; green; full gates.
- [ ] Commit: `feat(post-machine): lsp formatting via format() and CST document symbols`

---

### Task 9: Go-to-definition

**Files:**
- Create: `crates/post-machine/src/lsp/navigate.rs`; wire `mod navigate;` in `lsp/mod.rs`.

**Interfaces:** `definition(uri, pos)` — analysis-tier (returns None unless `state.analysis` is Some). Resolution order for the position:

1. **Call name**: the resolution entry whose span contains `pos` →
   - `Local { def_name_span }` → `DefTarget { uri: <same>, span: def_name_span }`;
   - `ImportBinding { use_span, full_path }` → `full_path` starting `std::` → the roster entry for `full_path` at `materialized_std_uri()` (`DefTarget { uri: <std uri>, span: entry.name_span }`); roster miss or materializer None → for std paths **null** (spec: IO failure degrades to null), for non-std paths → `DefTarget` at `use_span`;
   - `QualifiedExternal { full_path }` → `std::…` → materialized roster target (miss/None → null); anything else → null;
   - `Unresolved` → null.
2. **Label reference**: hit-test the Task 2 spans (`Goto.label_span`, `Check.marked_span`/`blank_span` when the arm is a `Label`, `succ_label_span`) inside the innermost enclosing function (CST walk via extent spans; labels are function-scoped) → the matching `Label.span` in that same function.
3. **`use` path naming `std::…`**: pos inside a `UsePath.span` whose `path[0] == "std"` → the roster entry for its full path in the materialized std (miss/None → null).
4. Anything else → null.

**Steps (TDD):**
- [ ] Failing tests (fixture from Task 3's shape, positions computed from the source text): local call → the definition's `name_span`; nested call → nested def; qualified-internal `@ns::inner()` → in-file `inner`; import-binding `@ext()` → the `use ext;` span; `@ge()` (bound to `std::goToEnd`) → uri is the materialized file (`starts_with("file://")`, exists on disk) and the span equals the roster's `name_span` for `goToEnd`; `@other::thing()` → None; `@mystery()` → None; `goto 1` reference → the `1:` label's span in the same function (and NOT a same-valued label in a sibling function — two-function fixture); a check arm reference → same; pos inside `use std::goToEnd`'s path → materialized std.
- [ ] Failing degradation test: same fixture with a trailing `goto 99` (post-parse fatal) → every `definition` call returns None (analysis-tier).
- [ ] Implement; green; full gates.
- [ ] Commit: `feat(post-machine): lsp go-to-definition off the resolution table + materialized std`

---

### Task 10: Completions

**Files:**
- Create: `crates/post-machine/src/lsp/complete.rs`; wire `mod complete;`.

**Interfaces:** `completion(uri, pos)` over the **current** significant tokens (WithComments minus `Comment`; lex failed → empty) and current CST for positioning, with the *names* roster from `analysis` or, when absent, `scopes_for_completion` (the sanctioned staleness). Prefix/replace rule: if an `Ident`/`Number` token's span contains the cursor (or ends exactly at it), that whole token span is `replace_span`; otherwise a zero-width span at the cursor. Every candidate sets `insert_text` (usually = label).

The four contexts, detected in this order (spec "Completions"):

1. **`use` path** — the current top-level statement starts with `Ident("use")` (walk back to the previous `Semi`/`LBrace`/`RBrace`): after a `ns::` prefix (a `ColonColon` immediately left of the prefix; collect the full chain of `Ident ::` back) → that namespace's members: `scopes.defs` under the exact path (Function kind) + child namespaces one segment deeper (Module kind) + the roster (names under `std::`); with no `::` → roots: the distinct first segments of `scopes.defs`/`scopes.bindings` keys (Module) + `std` (Module).
2. **Qualified call path** — a `ColonColon` immediately left of the prefix AND the chain walks back to an `At` → same member logic as context 1's prefixed case.
3. **Call position** — the token immediately left of the prefix is `At` → visible callables with shadowing (definition outranks import, inner outranks outer — first-wins per bare name, assembled in flatten's own resolve order): (a) nested defs of the enclosing function chain from the **current CST** (innermost-outward, hoisted); (b) per enclosing namespace prefix, longest first: `defs[prefix]` names then `bindings[prefix]` names (from the names roster); (c) the std roster as qualified paths (`std::goToEnd`, …). All Function kind. Enclosing chain/ns-path from CST extent spans; if the CST is unavailable (parse failed) fall back to the top-level scope (`[]`) and skip nested names.
4. **Command position** — cursor at a statement start / after a label `Colon` / after a `Comma`: the eight command words **cited from `parser::RESERVED`** (Keyword kind — never a hardcoded copy). After a `Comma` (inside a group): drop `goto` always; drop `check` and `halt` unless the slot is final (the next significant token at/after the cursor's token is not a `Comma` before the statement ends). After `Ident("goto")`: the enclosing function's labels instead (Value kind, the label number as text).

No context match → empty. Cross-file namespaces are deliberately invisible — only this file's scopes + the std roster, ever.

**Steps (TDD)** — one failing test per behavior, positions computed against fixture strings:
- [ ] Call position: `@` at top level offers `main`-level defs + imports + `std::…` roster paths; shadowing — a def and an import with the same bare name yield ONE candidate (the def); inner nested def shadows an outer name; nested defs of the enclosing chain are offered innermost-outward and hoisted (a nested fn defined *below* the cursor's statement still offered).
- [ ] `use` contexts: `use ` → namespace roots + `std`; `use std::` → the 11 routine names; `use ns::` → that namespace's members.
- [ ] Qualified call: `@std::` → routine names; `@ns::` → members.
- [ ] Command position: statement start → all eight `RESERVED` words; after `1:` label colon → same; after a comma with more items following → no `goto`/`check`/`halt`; after a comma in the final slot → `check`/`halt` present, `goto` still absent; after `goto ` → the enclosing function's labels only.
- [ ] Prefix replacement: cursor mid-word in `@he|lp` → `replace_span` covers the whole `help` prefix token; empty position → zero-width span.
- [ ] Staleness: clean doc, then a broken edit → call-position names still offered (from last-good scopes), while positions are computed against current tokens.
- [ ] Implement; green; full gates.
- [ ] Commit: `feat(post-machine): lsp completions — four contexts, shadowing, RESERVED-cited commands`

---

### Task 11: Code actions

**Files:**
- Modify: `crates/post-machine/src/lsp/mod.rs`.

**Interfaces:** `code_actions(uri, span)` — analysis-tier (lint only ran when analysis succeeded). Each stored lint finding with a `Fix` whose **diagnostic span overlaps** the request span (half-open overlap: `a.start < b.end && b.start < a.end`) yields `Action { title: fix.description, preferred: fix.applicability == MachineApplicable, edits: fix.edits.clone() }` — the CLI's `--fix` vs `--fix --force` distinction, LSP-natively. Compile warnings carry no fixes in v1 (their `fix` is always None — nothing to do).

**Steps (TDD):**
- [ ] Failing tests: (1) a fixture with an `unused-label` finding (MaybeIncorrect per the rule) → an action with `preferred: false`, title = the fix description; a `MachineApplicable` finding (pick a rule that emits one, e.g. `leading-zeros` if it does — check the rule table and use whichever fixture the lint suite already uses) → `preferred: true`; (2) request span NOT overlapping the finding → no actions; (3) **edits round-trip**: byte-apply the returned edits to the fixture (reuse `apply_fixes` or apply `Edit`s manually right-to-left) and re-run the service on the result → the finding is gone; (4) empty when analysis failed.
- [ ] Implement; green; full gates.
- [ ] Commit: `feat(post-machine): lsp quickfix code actions from lint fixes`

---

### Task 12: Semantic tokens

**Files:**
- Create: `crates/post-machine/src/lsp/tokens.rs`; wire `mod tokens;`.

**Interfaces:** `semantic_tokens(uri)` — analysis-tier: `state.analysis.as_ref()?` (null on any failed post-parse stage — one tier, never a resolution-free subset). Legend indices: types `namespace`=0, `function`=1, `number`=2; modifier bits `declaration`=1, `defaultLibrary`=2.

Emission (walk the CST + a `HashMap<Span, &Resolution>` built from the table):

- **Function definitions** (incl. nested): `name_span` → `function` + `declaration`.
- **Namespace declarations**: `name_span` → `namespace` + `declaration`.
- **`use` paths**: per-segment spans computed arithmetically from `UsePath.span.start` + the written segment lengths (+2 per `::`; single-line, ASCII identifiers — safe): all but the last segment → `namespace`; the last → `function`, + `defaultLibrary` when `path[0] == "std"`.
- **Resolved call names**: for each CST `Item::Call` whose `name_span` has a table entry ≠ `Unresolved`: split the *written* name on `::` (same arithmetic segmenting): non-final segments → `namespace`; final → `function`, + `defaultLibrary` when the resolution is `ImportBinding`/`QualifiedExternal` with a `std::…` full path. **Unresolved call names emit nothing** — the quiet visual cue complementing `undeclared-external`.
- **Labels**: definitions → `Label.span` *minus the trailing colon* (end col − 1) → `number` + `declaration`; references (the Task 2 spans; check arms only when the arm is `Label`) → `number`, bare.

Collect, sort by span start; debug_assert non-overlapping; return absolute `SemToken`s (the framework packs the wire encoding).

**Steps (TDD):**
- [ ] Failing tests — expected absolute token streams, hand-written per fixture: (1) a namespace + exported fn + nested fn + labels with goto/check/successor references + a `use std::goToEnd as ge;` + calls `@ge()` (final segment defaultLibrary), `@std::goToEnd()` (namespace `std` + function w/ defaultLibrary), `@local()` (plain function), `@mystery()` (ABSENT from the stream); assert the exact `Vec<SemToken>`; (2) label def token excludes the colon; (3) null while a post-parse stage fails (`goto 99` fixture); (4) a **drift guard**: the legend arrays and the emitter's constants are the same statics (emit indexes/bits ONLY via named consts defined next to the legend — the test asserts every emitted `token_type < legend.len()` and every modifier bit maps to a legend entry across a maximal fixture).
- [ ] Implement; green; full gates.
- [ ] Commit: `feat(post-machine): lsp semantic tokens — resolution-aware minimal legend`

---

### Task 13: `pmt lsp` subcommand, registry, e2e session, docs

**Files:**
- Create: `crates/post-machine/src/cli/lsp.rs`; modify `cli/mod.rs` (dispatch + USAGE), `completions/registry.rs` (+ its `#[cfg(test)]` name lists), `tests/completions_registry.rs` (`EXPECTED_TOP_LEVEL`), `tests/cli_programs.rs`.
- Create: `docs/lsp.md`; modify `docs/cli.md`, `README.md`.

**Interfaces (Produces):**

```rust
// cli/lsp.rs — the only place real stdio is handed over:
const LSP_USAGE: &str = "USAGE: pmt lsp\n\nRun the LSP server for .pmc on stdio until the client exits.\nExit code: 0 after shutdown/exit, 1 on exit without shutdown.\n";

pub(super) fn lsp(raw: &[String]) -> Result<CliOutput, String> {
    let mut args = Args::new(raw);
    if args.flag("--help") { return Ok(CliOutput::ok(LSP_USAGE.into(), String::new())); }
    let rest = args.positionals()?;
    if !rest.is_empty() { return Err(format!("lsp takes no arguments\n\n{LSP_USAGE}")); }
    let mut service = crate::lsp::PmcLanguageService::new();
    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout();
    let code = mtc_core::lsp::server::run(
        &mut stdin, &mut stdout, &mut service,
        mtc_core::lsp::server::ServerIdentity { name: "pmt lsp", version: env!("CARGO_PKG_VERSION") },
    );
    Ok(CliOutput { stdout: String::new(), stderr: String::new(), code: code as u8 })
}
```

`cli/mod.rs`: `mod lsp;`, a `Some("lsp") => lsp::lsp(&args[1..])` arm, and a `USAGE` line (`  lsp          run the LSP server on stdio`), placed with the other subcommands. Registry: `lsp_spec()` (`path: ["lsp"]`, `positional: Positional::None`, flags: `--help` only), inserted into `registry()`; update `top_level_help`, the registry's own `#[cfg(test)]` name lists, and `EXPECTED_TOP_LEVEL` in `tests/completions_registry.rs` — the drift guards force all of these.

**The e2e scripted session** (spec Testing, service bullet 8) — inline `#[cfg(test)]` in `src/lsp/mod.rs`, reusing core's `server::run` over in-memory pipes with the REAL service (mirror plan 1's `run_session` helper locally — ~20 lines): initialize → didOpen a bad file (an unused label + a `goto 99`… use a file whose first fatal is `undefined-label`) → assert the published diagnostic's span/code → didChange fixing the fatal → assert warnings+lint published → codeAction on the lint span → apply the returned edit client-side → didChange with the fixed text → assert diagnostics shrink → formatting round-trip → shutdown → exit code 0.

**Docs:**
- `docs/lsp.md` (new durable page): what the server serves; a capabilities table (feature × analysis tier × degradation); wiring samples for generic clients — Neovim (`vim.lsp.config`/`vim.lsp.enable` snippet launching `pmt lsp` for `pmc` filetype) and Helix (`languages.toml` entry); pointers to both shells under `editors/` (plan 3); the position-encoding note (UTF-16 on the wire, char-counted internally); the materialized-std explanation (cache path per platform, degradation to null); configuration (pmt.json + IDE settings, union semantics — cross-reference `docs/lint.md`).
- `docs/cli.md`: `## pmt lsp` section (USAGE block + prose: stdio, lifecycle exit codes, what it serves, pointer to `docs/lsp.md`), added to the top-level subcommand list.
- `README.md`: one feature line (LSP server + editor support) + `docs/lsp.md` link in the docs list.
- Release-notes version block (recorded in the PR/ledger, not the published docs): toolchain moved; `.pmc` language 0.2 unchanged; PM-1 dialect unchanged; IR 3 unchanged; containers unchanged. Fatal-code strings become stable identifiers as of this release.

**Steps (TDD):**
- [ ] Failing CLI tests in `tests/cli_programs.rs`: `pmt lsp --help` prints `LSP_USAGE`; `pmt lsp extra-arg` errors; `pmt --help` lists `lsp`.
- [ ] Registry: add `lsp_spec()`; run the completions drift suite — update the maintained lists until green; `zsh -n` smoke (`tests/completions_zsh.rs`) stays green.
- [ ] The e2e scripted session test (above) — write failing, then wire until green.
- [ ] Dogfood test (spec acceptance): `did_update` on the embedded `stdlib::SOURCE` → zero diagnostics; `semantic_tokens` → Some(non-empty); `format` returns text byte-equal to `SOURCE` (fmt-clean already — the fmt plan's dogfood pinned this).
- [ ] Docs written; forge-agnostic proofread (no issue numbers, no URLs beyond generic client docs).
- [ ] Full gates. Commit: `feat(cli): pmt lsp subcommand + docs — the .pmc language server ships`

---

## Self-check before handoff to plan 3

- All acceptance criteria the spec assigns to the server are demonstrable: fatal codes in both worlds; the four completion contexts; definition targets incl. the materialized std; quickfixes with preferred flags; semantic tokens per the legend; document symbols; formatting byte-identical to `pmt fmt`; config end-to-end (file + IDE union, mtime refresh, `--no-config`).
- Drift guards green: completions registry (now incl. `lsp`), `roster()` vs `object().symbols`, token legend vs emitter, fatal codes pairwise distinct.
- The three gates green; the dogfood test green.
