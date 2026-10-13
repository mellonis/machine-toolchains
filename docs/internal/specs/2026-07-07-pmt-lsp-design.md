# `pmt lsp` — the LSP server for `.pmc` — design

Date: 2026-07-07
Status: draft (review rounds of 2026-07-08 applied, including an
independent audit pass; awaiting final approval before planning)
Tracker: [machine-toolchains#4](https://github.com/mellonis/machine-toolchains/issues/4)

## Context

Issue #4 asks for one Rust LSP server (`pmt lsp`) reusing the real
lexer/parser/compiler, serving both VS Code and JetBrains: completions,
live diagnostics, go-to-definition, syntax highlighting (TextMate
grammar plus semantic tokens), and thin build/run wrappers in the
per-IDE shells.

The issue predates three shipped tracks that change its cost profile
dramatically — most of the hard substrate now exists:

- **Diagnostics primitives** (lint layer): `core::diagnostics` defines
  `Pos` (1-based line:col, columns count characters), `Span` (half-open
  range of `Pos`), `Diagnostic { code, span, message, fix }`, and
  `Fix { description, applicability, edits }` on the rustc model.
  `CompileError { span, kind }` carries a full span for every fatal.
  These map onto LSP `Diagnostic`/`CodeAction`/`TextEdit` almost 1:1.
- **The unified lossless CST** (fmt phase, option C/C1): `parse()` is
  now `parse_cst(tokens).map(lower_cst)`; the CST retains comments as
  trivia and every node's spans. The C1 decision explicitly built this
  tree to be shared with the LSP — this spec is that reuse.
- **`format(source) -> Result<String, CompileError>`**: the fmt
  library entry, designed from day one to double as the LSP's
  document-formatting provider.
- **`analyze()`** (`compiler.rs`): lex → parse → duplicate-binding
  check → flatten → IR lower, returning `AnalysisOutput { tokens, ast,
  ir, diagnostics, scopes }` — documented in-code as "everything the
  lint layer (and a future LSP) needs". `ScopeSummary` retains
  flatten's per-scope name maps (defs and import bindings keyed by
  namespace path).
- **`pmt lint`**: `LintReport` of `Diagnostic`s with machine-applicable
  and gated fixes — the in-editor quickfix inventory.

One structural fact keeps the whole design small: **`.pmc` is a
single-file language.** `use` declares an external symbol and binds a
bare name; resolution across objects happens at link time, never at
compile time. The server therefore needs no project model, no
workspace indexing, no cross-file invalidation — each open document is
a complete, independently analyzable compilation unit, plus one
well-known external library (the embedded stdlib).

`.pma` is the asymmetric side: `AsmError { line, kind }` is line-only
(no columns), assembly parsing is line-oriented with no CST, and
lint/fmt do not accept `.pma`. Serving `.pma` well is a parity project
of its own — filed as
[machine-toolchains#15](https://github.com/mellonis/machine-toolchains/issues/15)
(.pma CST + spans + lint + fmt). **This milestone is `.pmc`-only**;
the LSP gains `.pma` when #15 lands, through the same machinery.

## Decisions (settled with the user, 2026-07-07/08)

- **Feature set = full issue scope**: publish diagnostics (compile
  fatal + compile warnings + lint findings), document formatting,
  completions, go-to-definition, code actions from lint fixes,
  semantic tokens, and a TextMate grammar. Pulled in during spec
  review (2026-07-08): **document symbols** (outline/breadcrumbs/
  go-to-symbol/structure view) — the cheapest feature here, a pure
  CST walk.
- **Protocol stack = hand-rolled**, on `serde`/`serde_json` only.
  **Zero new Rust dependencies.** LSP over stdio is Content-Length
  framed JSON-RPC; the subset of protocol structs the feature set
  needs is bounded (~35 types) and versioned by the LSP spec itself.
  This extends the repo's no-clap ethos to the wire protocol.
- **`.pmc` only in v1**; `.pma` deferred to #15 (see Context).
- **Both editor shells ship this milestone**, in-repo under
  `editors/`: a VS Code extension and a JetBrains plugin built on
  **LSP4IJ** (Red Hat's open-source LSP client — works on Community
  editions too, unlike the native JetBrains LSP API). Shell extras:
  VS Code tasks + problem matcher, JetBrains run configurations.
  **No marketplace publishing** — both ship as sideloadable artifacts
  attached to GitHub releases.
- **Lint configuration ships in v1, from two sources** (settled
  during spec review, 2026-07-08): a **project file** — `pmt.json`,
  discovered as the nearest ancestor of the source file — read by
  BOTH the CLI and the LSP, plus **per-IDE settings UI** forwarded
  over the LSP configuration channel. Allow-lists from all sources
  merge by union. JSON, not TOML, follows from the zero-new-deps
  decision (TOML would need the first new crate; `serde_json` is
  already here). Also settled in the same review: **fatal error
  codes + CLI parity** (see the service section).
- **Code layout = framework in core** (the asm-framework pattern): the
  language-independent half (framing, JSON-RPC, protocol structs,
  document store, position mapping, server loop) lives in
  `core/src/lsp/` behind a `LanguageService` trait and carries zero
  PM-1 knowledge; the `.pmc` handlers and the `pmt lsp` subcommand
  live in `crates/post-machine`. The future `tmt lsp` reuses the
  framework verbatim. Core proves the boundary the same way the VM
  does: its framework tests run against a crate-private fake language
  service (the `test_arch` philosophy).

Alternatives considered and rejected: `tower-lsp` (async framework on
tokio + tower — the heaviest possible dependency footprint, and async
buys nothing for a single-file sync compiler); `lsp-server` +
`lsp-types` (pragmatic, but the first departure from the dependency
policy for code we can comfortably own); everything-in-post-machine
(defers the core/arch split until `tmt` exists, buying nothing now and
costing extraction churn later — the workspace already made the
opposite bet for the asm framework); a third `mtc-lsp` crate (a new
workspace member for one module's worth of code).

## Architecture

### Crate layout

```
crates/core/src/lsp/
  transport.rs   Content-Length framing over generic BufRead/Write
  jsonrpc.rs     request/response/notification envelopes, ids, errors
  types.rs       the protocol structs the feature set consumes
  position.rs    Span (1-based, char cols) <-> Position (0-based, UTF-16)
  docstore.rs    uri -> { version, text }, full-sync updates
  server.rs      blocking loop, lifecycle, dispatch to LanguageService
crates/post-machine/src/lsp/
  mod.rs         PmcLanguageService (implements the trait)
  complete.rs    completion contexts + candidates
  navigate.rs    go-to-definition (resolution table + std materializer)
  tokens.rs      semantic-token emission from CST + resolutions
crates/post-machine/src/cli/lsp.rs
                 `pmt lsp` — stdio + PmcLanguageService + server loop
editors/
  grammars/pmc.tmLanguage.json   single-source TextMate grammar
  vscode/                        VS Code extension (TypeScript)
  jetbrains/                     JetBrains plugin (Kotlin/Gradle, LSP4IJ)
```

### The `LanguageService` seam

The trait deliberately speaks the **toolchain's currency**, not the
protocol's: handlers take and return `core::diagnostics::Span`,
severity-wrapped diagnostics, plain strings, and absolute
semantic-token records. The framework owns every protocol conversion
in one place:

- 1-based char-counted columns ↔ 0-based UTF-16 code-unit columns
  (conversion walks the stored document line; positions past
  end-of-line or end-of-file clamp, per LSP);
- `Span` ↔ `Range`, inbound request positions included;
- severity/source/code onto wire diagnostics;
- absolute semantic tokens → sorted, relative-packed data array (the
  wire format's deltaLine/deltaStart encoding — unrelated to the
  `full`-vs-`delta` request flavor, which v1 does not offer);
- full replacement text → a whole-document `TextEdit`.

Trait shape (signatures illustrative, frozen at plan time):

```rust
pub trait LanguageService {
    fn language_id(&self) -> &str;                    // "pmc"
    fn trigger_characters(&self) -> &[char];          // ['@', ':']
    fn token_legend(&self) -> (&[&str], &[&str]);     // types, modifiers
    fn did_update(&mut self, uri: &str, text: &str)
        -> Vec<ServiceDiagnostic>;                    // -> publish
    fn did_close(&mut self, uri: &str);
    fn did_change_config(&mut self, settings: serde_json::Value);
    fn watched_globs(&self) -> &[&str];               // ["**/pmt.json"]
    fn completion(&mut self, uri: &str, pos: Pos) -> Vec<Candidate>;
    fn definition(&mut self, uri: &str, pos: Pos) -> Option<DefTarget>;
    fn code_actions(&mut self, uri: &str, span: Span) -> Vec<Action>;
    fn document_symbols(&mut self, uri: &str) -> Option<Vec<SymbolNode>>;
    fn semantic_tokens(&mut self, uri: &str) -> Option<Vec<SemToken>>;
    fn format(&mut self, uri: &str) -> Option<String>; // full new text
}
```

`ServiceDiagnostic` wraps a span + message with severity, source, and
optional code (core's `Diagnostic` has no severity/source — those are
presentation, chosen by the service). `DefTarget` is a target uri +
span, so definitions may point outside the requesting document (the
materialized stdlib). The framework's `docstore` owns raw text; the
service keeps its own per-URI analysis cache.

Configuration flows through the same seam: the framework feeds
`initializationOptions` and `workspace/didChangeConfiguration`
payloads to `did_change_config` as opaque JSON, registers a client
file watch for the service's `watched_globs()`, and after either a
config change or a watched-file change re-runs `did_update` over
every open document (it owns the text) so diagnostics republish —
the service never talks protocol.

### Runtime model

Single-threaded, synchronous, blocking read loop on stdio. Messages
are processed strictly in order; there is no async runtime, no worker
threads, and no request cancellation — `$/cancelRequest` is legally
ignored because every handler is micro-fast at `.pmc` scale (a full
re-analysis of a source file is microseconds-to-low-milliseconds, and
`analyze()` stops before the optimizer). Document sync is
**full-text** (`TextDocumentSyncKind::Full`); incremental sync is
complexity with zero payoff at these file sizes.

Lifecycle per the LSP spec: requests before `initialize` are answered
`ServerNotInitialized` (-32002); notifications before `initialize`
are dropped (except `exit`); `shutdown` then `exit` exits 0; `exit`
without `shutdown` exits 1. Unknown requests get `MethodNotFound`;
unknown notifications (including all `$/…`) are silently dropped, as
the spec requires.

Advertised capabilities: full+openClose text sync; completions with
trigger characters `@` and `:`; definition; document formatting;
code actions (kind `quickfix`); semantic tokens (`full` only, no
delta); document symbols; `positionEncoding: "utf-16"`. After
`initialized`, the
server registers a `workspace/didChangeWatchedFiles` watch for the
service's globs (`**/pmt.json`). `serverInfo` reports `pmt lsp` +
the crate version.

Position encoding note: `Pos.col` counts characters — Unicode scalar
values, i.e. UTF-32 code units — so a future `utf-32` negotiation
(LSP 3.17 `positionEncoding`) would make the conversion the identity.
v1 always converts to UTF-16 (the only encoding every client
supports); the framework isolates the conversion so switching per
client capability later is a framework-only change.

### Error containment

Per-message dispatch wraps in `catch_unwind`: a handler panic returns
`InternalError` carrying the panic message, logs to stderr, and the
loop keeps serving — one bug must not kill the editor session. This
is safe precisely because the state is thin: the doc store is rebuilt
by the client's next full-sync change, and the service's analysis
cache is recomputed from text. Malformed JSON gets a JSON-RPC
`ParseError` response with a null id. A failure to materialize the
stdlib file (unwritable cache dir) degrades to "std definitions
return null", never an error.

## The `.pmc` language service

### Staged analysis, honest degradation

On every `didOpen`/`didChange` the service re-runs the real front
half — exactly today's `analyze()` stages: `lex_with(WithComments)`
→ `parse_cst` (+ `lower_cst`) → duplicate-binding check → `flatten`
→ `ir::lower` — and caches, per URI, the outcome of each stage.
Precision matters here: `flatten` itself is infallible; the
post-parse fatals come from the binding check (`DuplicateBinding`)
and from lower (`UndefinedLabel`), and lower also contributes the
`unreachable-code` warning — the pipeline must run through
`ir::lower`, never stop at flatten, or `goto 99` to a missing label
would show no error in the editor.

```
DocState {
  text, version,
  tokens,      // present unless lexing failed
  cst, ast,    // present unless parsing failed
  analysis,    // present unless a post-parse stage failed:
               //   scopes, warnings, resolutions
  lint,        // present when analysis is present: LintReport + fixes
  fatal,       // Option<CompileError>
}
```

Features degrade by stage rather than answering from stale text:

- **formatting** and **document symbols** need a parsed CST *of the
  current text* — parse failed means null (the TextMate grammar
  keeps static coloring alive; nothing mispositions);
- **semantic tokens**, **go-to-definition**, and **code actions**
  need `analysis` of the current text — call-site tokens and
  `defaultLibrary` modifiers come from resolutions, so a failed
  post-parse stage means null/empty (clients keep the previous
  tokens or TextMate colors; one tier per feature, never a
  membership-shifting subset stream);
- **completions** position their context against the current tokens/
  CST, but may take the *symbol roster only* (candidate names) from
  the last-good `analysis` — names stay useful mid-edit and no
  positions are involved. This is the single deliberate staleness
  exception.

`analyze()` today is fail-fast (first `CompileError` aborts). That is
kept: one honest error beats a cascade of guesses. What the service
needs beyond today's `analyze()` is (a) the stage split above — parse
success retained even when a post-parse stage fails — (b) the
resolution table below, and (c) fatal error codes. All are additive
reshapes.

### Diagnostics: one publish, three channels

Every `did_update` publishes the merged set for that document
version — a publish always replaces the document's whole diagnostic
set. Coverage follows the pipeline: a fully analyzed file gets
whole-file warnings + lint findings all at once; a file with a
fatal shows **exactly one error, the first** (fail-fast, no parser
error recovery — fixing it reveals the next on the next analysis):

| channel            | severity | source     | code            |
|--------------------|----------|------------|-----------------|
| fatal CompileError | Error    | `pmt`      | carried (`duplicate-label`, …) |
| compile warnings   | Warning  | `pmt`      | carried (`unused-import`, …) |
| lint findings      | Warning  | `pmt lint` | carried         |

Lint runs whenever analysis succeeds, with the effective allow-list
resolved from the project file and IDE settings (see Configuration).
Lint `Fix`es are retained per-URI for the code-action handler.
`didClose` drops the state and publishes an empty diagnostic set.

### Fatal error codes (with CLI parity)

Compile warnings and lint findings carry stable code strings by
construction; fatal `CompileErrorKind` variants have none today — no
surface (CLI included) has ever printed one. Decided during spec
review: **fatals get codes in this milestone, and the CLI shows them
too.**

- `CompileErrorKind::code() -> &'static str` — one kebab-case code
  per variant (`duplicate-label`, `goto-return`,
  `empty-builtin-parens`, …), named in a deliberate pass at plan
  time; the match is exhaustive by construction and a test asserts
  the codes are pairwise distinct.
- The LSP fills `Diagnostic.code` with it (table above).
- **CLI parity via `Display`**: `CompileError`'s rendering gains a
  bracketed suffix — `line 3:7: duplicate label \`5\`
  [duplicate-label]` (quoting the real `Display` text). Because
  every renderer (compile
  errors, lint's fatal passthrough, fmt's parse errors) goes through
  `Display`, one change gives every surface the code at once.
  Blast radius: tests and docs that assert exact rendered error text
  update in the same change.
- Lint's own finding renderer stays message-only this milestone (its
  codes already have a CLI surface via `--allow` and the docs/lint.md
  catalog); a uniform bracketed-code sweep over lint output is a
  separate cosmetic decision.
- Codes are user-visible identifiers: once shipped they are stable —
  renames are breaking documentation changes. A compile-errors table
  in `docs/cli.md` lists them (settled at audit — one home, not an
  either/or).

### The resolution table (a compiler extension)

`flatten` already resolves every call site — innermost-outward
through nested defs, then per enclosing namespace prefix defs, then
import bindings — it just doesn't record the outcome. It gains an
additive side table on `AnalysisOutput`:

```rust
resolutions: Vec<(Span /* call name span */, Resolution)>

enum Resolution {
    Local { def_name_span: Span },        // fn in this module (incl. nested)
    ImportBinding { use_span: Span, full_path: String },
    QualifiedExternal { full_path: String }, // @ns::name — self-declaring
    Unresolved,                            // bare undeclared external
}
```

Recording checks a qualified path against the module's
defs-by-full-name before falling through: `@ns::name()` whose target
this file defines records `Local { def_name_span }` — the compiler
already proves such calls internal (its reachability pass builds
edges from them) — and only genuinely external qualified calls
record `QualifiedExternal`.

This is the **single source of truth** for go-to-definition and for
the resolution-aware semantic-token modifiers; the LSP layer never
re-implements scope walking.

Label references are a separate, span-shaped problem: today the AST
carries spans only for label *definitions* — `Goto` has no span for
its target, check arms and successors are bare values, and
`succ_span` covers the whole paren range. The parser therefore
gains **additive reference spans** on goto targets, check arms, and
successor labels, rippling mechanically through the CST's verbatim
item embedding; go-to-definition hit-testing and label semantic
tokens read them directly, and label-to-definition resolution stays
per-function (no flatten change). The alternative — scanning raw
tokens for number positions near an item — was rejected as
corner-case-prone re-derivation.

### The stdlib roster helper

Closes a known gap: the stdlib is exposed only as `SOURCE` text and a
compiled `object()`. A new `stdlib::roster()` parses `SOURCE` once
(OnceLock) into entries `(full_path, name_span, decl_line)` for the
11 exported routines. Consumers: completions (the `std::` candidate
list) and go-to-definition (spans inside the materialized std file).
A drift test asserts the roster's names equal the exported symbol
names on `object()`.

### Completions

Context is detected from the CST/tokens at the cursor; four contexts:

1. **Call position** (after `@`): callables visible from the cursor's
   scope — nested defs of the enclosing function chain (hoisted,
   innermost-outward), defs of the enclosing namespace scopes and
   import bindings (both from `ScopeSummary`, keyed by ns path), and
   the `std::` roster as qualified paths. Shadowing is respected:
   only the visible binding for a bare name is offered (definition
   outranks import, inner outranks outer).
2. **`use` path**: after `use ` — the file's namespace roots plus
   `std`; after a `ns::` prefix — that namespace's members
   (`ScopeSummary` defs under the path; the roster under `std::`).
3. **Command position** (statement start, after a label colon, after
   a comma in a group): the eight reserved command words, cited from
   `parser::RESERVED` — never a hardcoded copy (the completions-
   registry anti-drift principle). After `goto `, additionally the
   enclosing function's labels. Inside a comma group the offer
   filters to positionally legal commands — `goto` never appears in
   a group, `check`/`halt` only in the final slot (the parser's
   `GroupPosition` rules; never offer code that cannot parse).
4. **Qualified call path** (after `::` in `@…`): namespace members,
   as in context 2.

Cross-file namespaces are deliberately invisible (settled during
spec review, 2026-07-08): completion and navigation offer only what
the compiler can prove from the current file plus the embedded
stdlib — a namespace defined in another file (directories mean
nothing to the toolchain) completes to nothing, and its qualified
calls stay typable, self-declaring, and link-time-resolved, exactly
as at the CLI. Making cross-file exact is the project-manifest
follow-up in the ledger; guessing a link set from workspace scans
was considered and rejected.

Item kinds: Function / Module (namespace) / Keyword (commands) /
Value (labels). Every item inserts via `textEdit` over the exact
token prefix, so replacement never depends on client-side word
heuristics.

### Go-to-definition

Read off the resolution table:

- `Local` → the definition's `name_span` in the same document;
- `ImportBinding` → the binding `use` line — unless the bound path is
  `std::…`, which jumps into the **materialized stdlib**;
- `QualifiedExternal` of `std::…` → materialized stdlib (qualified
  calls whose target this file defines were already recorded as
  `Local` and jump in-file);
- other externals and `Unresolved` → null;
- label reference → the label's defining span;
- a `use` path naming `std::…` → materialized stdlib.

The materialized stdlib: the embedded `std.pmc` is written once per
toolchain version to a cache path — `$XDG_CACHE_HOME/pmt/<version>/std.pmc`,
falling back to `~/.cache`, `%LOCALAPPDATA%` on Windows — on first
demand, and definitions return a plain `file:` URI into it at the
roster's spans. Client-agnostic (no custom URI schemes, no virtual-
document providers), works identically in every editor. Any IO
failure degrades to null.

### Document symbols

`textDocument/documentSymbol` returns the hierarchical symbol tree —
the one response that powers the Outline panel, breadcrumbs, and
go-to-symbol in VS Code and the Structure view in JetBrains. A pure
CST walk, no analysis stage: namespace blocks (kind Namespace, one
node per block — reopened blocks stay separate siblings, as in
source) containing functions (kind Function), nested functions as
children of their parent. Labels are not emitted (kept for a
possible follow-up; they'd make goto-heavy outlines noisy).
`selectionRange` is the declaration's `name_span`; `range` is the
node's full span. Because it needs only a parse, the outline keeps
working while post-parse analysis fails.

### Code actions

Lint fixes become `quickfix` actions for the requested range: each
stored `Fix` whose diagnostic span overlaps the range yields a
`CodeAction { title: fix.description, kind: quickfix, edit }` with
the fix's `Edit` spans converted to text edits on the document.
`isPreferred = true` for `MachineApplicable`; `MaybeIncorrect` fixes
are offered but not preferred — the CLI's `--fix` vs `--fix --force`
distinction, expressed LSP-natively. Compile warnings carry no fixes
in v1.

### Semantic tokens

A deliberately minimal legend that only adds what static TextMate
coloring cannot know — resolution:

- types: `namespace`, `function`, `number`;
- modifiers: `declaration`, `defaultLibrary`.

Emitted: function names at definition sites (`function` +
`declaration`, nested included) and at resolved call sites
(`function`, plus `defaultLibrary` when resolution is `std::…`);
namespace segments in declarations, `use` paths, and qualified calls
(`namespace`, `declaration` on the block name); the final segment of
a `use` path (`function`, `defaultLibrary` if std); labels (`number`,
`declaration` at the definition, bare at references). Unresolved
call names are deliberately *not* tokenized — they keep the plain
identifier color, a quiet visual cue that complements the
`undeclared-external` warning. Keywords, comments, operators, and
literal numbers outside label positions stay TextMate's job. Tokens
are emitted sorted and non-overlapping; the framework packs the wire
encoding. An analysis-tier feature: on a failed post-parse stage the
response is null (see the degradation list) — never a
resolution-free subset.

### Formatting

`textDocument/formatting` calls the same `format()` as `pmt fmt`.
Changed output returns one whole-document `TextEdit` (no diffing —
simple and correct; clients apply it atomically); unchanged output
returns an empty edit list; unparseable text returns null (quiet — no
error toast; the parse error is already on screen as a diagnostic).
Range formatting is not offered. The LSP thereby fulfills the fmt
spec's "IDE/LSP note": fmt integrates as the document-formatting
provider, never as per-position diagnostics.

## Configuration: project file + IDE settings

`.pmc` projects have no manifest; this milestone introduces the
toolchain's first project config file, deliberately tiny.

### The file: `pmt.json`

```json
{ "lint": { "allow": ["unused-label", "shadowed-import"] } }
```

- **Format**: plain JSON, parsed with `serde_json` — the format is a
  consequence of the zero-new-deps decision (TOML would cost the
  first new crate). No comments in v1 (a JSONC stripper is a
  possible follow-up, in the ledger).
- **Discovery**: nearest-ancestor walk from the source file's
  directory to the filesystem root, first `pmt.json` wins — the
  rustfmt.toml model, settled during spec review (2026-07-08):
  **nearest-wins, never a cascade** — an ancestor `pmt.json` above
  the winning one is ignored, not merged (union applies across
  *sources* — file/IDE/flags — not across nested files; a
  cascade-with-root-marker was considered and rejected for this
  milestone). Per source file, so both the CLI (many inputs per
  run) and the LSP (per open document) use the same rule. Documents
  with no filesystem path (`untitled:`) get no project config.
- **Schema**: `lint.allow: [string]` is the only key in v1. Unknown
  keys and unknown allow codes are rejected, not ignored — a typo
  must not silently disable itself. `fmt` stays configless by
  design (the gofmt model); nothing else has knobs.
- **Semantics**: allow-lists **merge by union** across sources —
  defaults ∪ project file ∪ IDE settings (LSP) or `--allow` flags
  (CLI). An allow-list only ever suppresses findings, so union is
  the predictable reading; there is no override ordering to learn.

### CLI side

`pmt lint` discovers and applies `pmt.json` per input file. A new
`--no-config` flag ignores project files entirely (CI runs that want
flag-only behavior). Invalid config — unparseable JSON, unknown key,
unknown allow code — is a hard per-file error, same posture as the
existing unknown `--allow` code error. The completions registry
entry for `lint` gains `--no-config`.

### LSP side

- The service re-runs the nearest-ancestor walk per analysis (a few
  stats — correctness requires it: a newly created `pmt.json` in a
  nearer directory must win), with a `(path, mtime)` cache that only
  skips re-parsing an unchanged winner. Effectively free at this
  scale.
- IDE settings arrive through the standard channel:
  `initializationOptions` at startup, `workspace/
  didChangeConfiguration` live. The framework passes both to the
  service through one generic hook — `did_change_config(settings:
  serde_json::Value)` — keeping core language-agnostic; the service
  reads `{ "lint": { "allow": [...] } }`, re-lints open documents,
  republishes.
- The server registers a client file watch for `**/pmt.json`
  (`workspace/didChangeWatchedFiles`); on change the service drops
  its config cache, re-analyzes open documents, republishes. Clients
  without watch support still pick changes up on the next edit via
  the mtime cache.
- Invalid config in the LSP surfaces as one Warning diagnostic
  (code `invalid-config`, source `pmt`) at 1:1 of each affected
  document — visible where the user is looking, since the server
  does not serve `pmt.json` itself. Lint then runs with the
  remaining valid sources.

### Editor settings UI

- **VS Code**: `contributes.configuration` exposes `pmt.lint.allow`
  (string array) in the standard Settings UI, user- or
  workspace-scoped; the client forwards changes live.
- **JetBrains**: the plugin's settings page (which already holds the
  `pmt` binary path) gains the allow-list field; LSP4IJ pushes the
  change to the server.

## CLI: `pmt lsp`

No flags in v1. Runs the server on stdio until `exit`; the process
exit code follows the LSP lifecycle (0 after `shutdown`, 1 on `exit`
without it). Gets a USAGE line in `cli/mod.rs`, a
`completions::registry` entry (the registry drift-guard test forces
this), and a `docs/cli.md` section. Library code never prints: the
server loop writes protocol frames to the writer it is handed and
panics/log lines to stderr; `cli/lsp.rs` hands it real stdio.

Version spaces: adding `pmt lsp` moves the **toolchain version
only** — `.pmc` language, PM-1 `.pma` dialect, `IR_VERSION`, and the
container formats are all `unchanged` in the release-notes version
block. (Fatal error codes change rendered error *text*, which is not
a versioned contract; the code strings themselves become stable the
release they ship.)

## Editor shells

### Shared TextMate grammar

`editors/grammars/pmc.tmLanguage.json`, scope `source.pmc`: line and
block comments, the eight command words, `use`/`namespace`/`export`,
labels and numbers, `@` call sigils and callee paths, `::`
separators, punctuation. Single-sourced: VS Code references it by
relative path; the JetBrains build copies it in at `buildPlugin`
time. No second copy is ever committed.

### VS Code (`editors/vscode/`)

TypeScript extension on `vscode-languageclient` (npm-side
dependencies are outside the Rust-crate policy — noted here
deliberately). It contributes:

- the `pmc` language (extensions `.pmc`) + the shared grammar;
- a client launching `pmt lsp` (setting `pmt.path`, default `pmt` on
  `PATH`) and forwarding configuration (`pmt.lint.allow`, see
  Configuration) at startup and live on change;
- a `pmt` **problem matcher** for `file:line:col: message` output;
- a **task provider** (type `pmt`) generating file-scoped tasks for
  the active `.pmc` document: compile, lint, `fmt --check`. Full
  build-and-run pipelines (compile → link → run with a tape) are
  user-authored `tasks.json`; the extension README documents a
  ready-to-paste snippet wired to the problem matcher.

Packaged with `vsce package` into a sideloadable `.vsix`, attached to
GitHub releases. No marketplace publishing this milestone.

### JetBrains (`editors/jetbrains/`)

Kotlin/Gradle plugin depending on the **LSP4IJ** plugin
(`com.redhat.devtools.lsp4ij`) — works on Community editions, richer
LSP coverage than the native platform API. It registers:

- the `.pmc` file type;
- a `LanguageServerFactory` with a process connection launching
  `pmt lsp` (binary path and the lint allow-list on the plugin
  settings page, pushed to the server on apply);
- the shared TextMate grammar for static highlighting via the
  platform TextMate bundle support, with LSP4IJ's semantic-token
  layer on top (exact registration mechanism is an implementation
  detail for the plan; the contract is: the grammar file is the
  shared one, copied at build);
- a **run-configuration type** wrapping `pmt`: subcommand preset
  (compile / lint / run), free-form arguments, working directory —
  thin process wrappers, no build-system ambitions.

Built with `gradle buildPlugin` into a sideloadable zip, attached to
GitHub releases. No marketplace publishing this milestone.

### Version compatibility (plugins ↔ toolchain ↔ language)

The shells are **version-thin clients** — the load-bearing rule of
this section: no plugin contains language knowledge. Neither shell
parses `.pmc`, reads `pmt.json`, or knows `PMC_LANG_VERSION`; the
acceptance contract is enforced solely by the `pmt` binary the user
installed, and every language-versioned behavior (diagnostics,
completions, navigation, formatting, semantic tokens) arrives over
the wire from it. Upgrading the language means upgrading `pmt`, not
the plugins.

Skew between a plugin and a binary is absorbed in layers:

- **Protocol**: the `initialize` capability negotiation gives both
  directions of skew the intersection of what client and server
  speak — LSP's own design, no work for us.
- **Cosmetics**: the TextMate grammar is the single language-coupled
  artifact a plugin ships, and it is cosmetic-only — after a grammar
  change an old plugin may color a new construct plainly until
  updated, while the server's semantic tokens keep the
  resolution-aware layer correct. Grammar lag is acceptable by
  construction; correctness never depends on it.
- **Output format**: the VS Code problem matcher tracks CLI error
  rendering, not the language — its regex accepts the bracketed
  fatal-code suffix from day one.

Skew policy: on startup the client checks the binary's version
(`pmt --version`; `serverInfo.version` confirms in the handshake).
Older than the plugin's declared minimum-tested version → a clear
notification naming both versions and the fix — **warn, not
block** (a pre-LSP binary already fails loudly: no `lsp`
subcommand, actionable error). Each shell versions independently
(its own semver), ships alongside toolchain GH releases, and its
README states the tested `pmt` range.

Both shell READMEs document installing the server:
`cargo install --path crates/post-machine` (or any release binary on
`PATH`). Node and Gradle toolchains are needed only under `editors/`
and only by people touching the shells; the Rust workspace remains
dependency-clean.

## Contracts

- **Core carries zero PM-1 and zero `.pmc` knowledge.** The framework
  compiles, runs, and is fully exercised against a crate-private fake
  `LanguageService`; nothing in `core/src/lsp/` names a `.pmc`
  concept. (The `test_arch` philosophy, applied to the second
  framework.)
- **Zero new Rust dependencies.** `serde`/`serde_json` remain the
  only runtime deps; the protocol structs are limited to what the
  advertised capabilities consume — no dead protocol surface.
- **No stale positions.** No feature ever returns positions computed
  against text other than the document's current version. The single
  sanctioned staleness is completion *names* from the last-good
  analysis.
- **Single-source symbol logic.** Call resolution lives in `flatten`
  (recorded in the resolution table); the command vocabulary is
  `parser::RESERVED`; the std roster derives from the embedded
  `SOURCE` and is drift-tested against `object().symbols`. The LSP
  layer never re-implements any of them.
- **Formatting is `format()`.** Byte-identical to `pmt fmt` by
  construction (same function); the server adds no formatting logic.
- **Fail soft, stay up.** Handler panics are contained per request;
  degraded stages return null/empty, never guesses.
- **Thin-renderer rule upheld.** The server loop is library code that
  writes to injected Read/Write handles; only `cli/lsp.rs` touches
  real stdio.

## Testing

No CI exists; the bar is the three local gates (`cargo test
--workspace`, clippy `-D warnings`, `cargo fmt --check`).

- **Core framework** (`crates/core`, fake-service):
  - transport: framing round-trips, split/partial reads, unknown
    headers tolerated, missing Content-Length rejected;
  - jsonrpc: envelope serde (number and string ids), error codes;
  - position mapping: char-col ↔ UTF-16 across ASCII, Cyrillic,
    astral/emoji (surrogate pairs), clamping past line/file end —
    property-tested with proptest (round-trip on valid positions);
  - semantic-token wire packing (relative line/col) against
    hand-computed vectors;
  - a scripted full session through in-memory pipes with the fake
    service: initialize → didOpen → publishDiagnostics → completion/
    definition/formatting round-trips → didChange →
    didChangeConfiguration (asserting the framework re-publishes all
    open docs) → didChangeWatchedFiles (same) → shutdown → exit,
    asserting exit codes and that requests-before-initialize error
    correctly.
- **Post-machine service** (fixture `.pmc` sources):
  - diagnostics merge: fatal-only when parse fails; warnings + lint
    together when analysis succeeds; didClose clears;
  - each completion context, incl. shadowing (definition over import,
    inner over outer), nested-fn hoisting, `std::` qualification,
    label candidates after `goto`;
  - definition targets: local, nested, import binding, std
    (materialized file exists and span lands on the routine name),
    qualified-internal → in-file def, qualified-external null,
    label refs;
  - code actions: edits byte-applied to the fixture re-lint clean;
    preferred flag tracks applicability;
  - semantic tokens: expected absolute token streams per fixture;
    unresolved call names absent from the stream; null while a
    post-parse stage fails;
  - formatting: output equals `fmt::format` on the same input;
  - document symbols: expected tree per fixture (namespaces with
    reopened siblings, nested functions as children, no labels),
    and non-null while post-parse analysis fails;
  - configuration: nearest-ancestor discovery (nested dirs, no file,
    stop at root), union merge of file + settings, mtime-cache
    refresh, invalid config → CLI hard error vs LSP `invalid-config`
    diagnostic at 1:1, `--no-config` ignores the file;
  - one end-to-end scripted session with the real service through
    the framework loop (bad file → expected error span → apply a fix
    → diagnostics shrink).
- **Drift guards**: completions registry gains `lsp` (existing test
  enforces exactness); `stdlib::roster()` vs `object().symbols`;
  token legend vs the set of types/modifiers the emitter uses;
  fatal error codes pairwise distinct (exhaustiveness is free — the
  `code()` match is over the enum).
- **Shells**: no automated editor e2e in v1. Each shell README
  carries a manual test checklist (open file → squiggles, completion,
  jump-to-def incl. std, quickfix, format-on-save, task/run-config
  smoke).

## Documentation

- `docs/lsp.md` (new durable page): what the server serves, a
  capabilities table, wiring instructions for generic LSP clients
  (Neovim and Helix samples), pointers to both shells, the position-
  encoding note, the materialized-std explanation.
- `docs/cli.md`: the `lsp` subcommand section; `lint` gains
  `--no-config`; the compile-errors table (code per
  `CompileErrorKind`) and the bracketed-suffix rendering note.
- `docs/lint.md`: the `pmt.json` project file (schema, discovery,
  union semantics).
- `README.md`: feature list line + docs link.
- Shell READMEs: install, sideload, settings, manual checklist,
  tasks/run-config usage.
- Published-docs policy applies throughout: no issue/PR numbers, no
  forge URLs in `README.md`/`docs/` — substance in prose.

## Acceptance criteria

- Fatal errors carry their kebab-case code in both worlds: the LSP
  `Diagnostic.code`, and every CLI rendering of a `CompileError`
  (compile, lint passthrough, fmt) shows the bracketed suffix.
- `pmt lsp` on stdio serves, against a real client: publish
  diagnostics on open/change/close for all three channels; the four
  completion contexts; go-to-definition for local/nested/import/std/
  label targets; lint quickfixes with correct preferred flags;
  semantic tokens per the legend; the document-symbol outline;
  whole-document formatting byte-identical to `pmt fmt`.
- The three local gates pass; core's LSP tests never reference `.pmc`
  concepts; all drift guards green.
- The VS Code `.vsix` and JetBrains plugin zip build from a clean
  checkout (npm/gradle respectively), sideload, and pass their manual
  checklists against the same `pmt` binary.
- The dogfood file: opening the reformatted `std.pmc` under the
  server yields zero diagnostics, full semantic tokens, and
  format-no-op.
- Configuration end-to-end: a `pmt.json` allow-list suppresses the
  same finding in `pmt lint` and in the editor; changing the IDE
  setting re-publishes without restart; editing `pmt.json` on disk
  re-publishes via the file watch.
- Release-notes version block: toolchain moved; language / PM-1
  dialect / IR / containers `unchanged`.

## Out of scope (the follow-up ledger)

- **`.pma` support** — #15 (parity: CST, spans, lint, fmt), then the
  LSP picks it up through the same `LanguageService`.
- **Hover** (resolved full name, std routine doc) and **signature
  help** — natural next server features. Their doc *source* is the
  function-documentation language proposal (doc lines + a
  `[deprecated]` attribute), filed as
  [machine-toolchains#17](https://github.com/mellonis/machine-toolchains/issues/17).
- **Labels in the outline** — child symbols under each function;
  deferred to keep goto-heavy outlines quiet (documentSymbol itself
  shipped in v1).
- **Rename / find-references** — needs a references index the
  resolution table almost provides; deliberate v2 candy.
- **Config growth**: comments in `pmt.json` (a small JSONC stripper),
  further keys (compile `-Werror`, future tool sections). `fmt`
  stays configless by design, permanently.
- **Project manifest / declared link set** — `pmt.json` is lint-only
  today; growing it into a manifest that declares which sources/
  objects link together would make cross-file completion,
  navigation, and eventually rename *exact* instead of guessed, and
  would be shared by `pmt link`, the LSP, and editor build/run
  tasks (the same gap that keeps the VS Code run pipeline a
  documented snippet). Filed as
  [machine-toolchains#16](https://github.com/mellonis/machine-toolchains/issues/16).
- **Incremental text sync, pull diagnostics, semantic-token deltas,
  request cancellation** — perf machinery a single-file language
  doesn't need yet.
- **Marketplace publishing** (VS Code Marketplace / OpenVSX /
  JetBrains Marketplace) — accounts, tokens, release CI; separate
  decision.
- **`positionEncoding` negotiation (utf-32/utf-8)** — framework-only
  change when wanted.
- **lint-via-stdin (`--stdin-filename`)** — the fmt spec parked it
  "tied to the LSP"; the LSP's live lint channel now covers the
  in-editor case, so the CLI flag stays unfiled unless a pipeline
  asks for it (the remaining audience: staged-content hooks,
  `git show :f.pmc | pmt lint -`). Design note for then:
  `--stdin-filename` doubles as the `pmt.json` discovery anchor
  (reviewed 2026-07-08, kept deferred).
- **Parser error recovery** — surfacing multiple simultaneous
  fatals instead of first-error-only. A major parser reshape
  (recovery nodes in the CST, cascade suppression) with a known
  failure mode (misleading follow-on errors); v1 deliberately keeps
  the fail-fast single-error model, reviewed 2026-07-08.
- **C2 typed CST views** (#14) — v1 works on C1 as-is; revisit on
  perf pressure or if handler ergonomics demand views.
