# `pmt lsp` Plan 1/3 — the language-agnostic LSP framework in `mtc-core`

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** A complete, language-agnostic LSP server framework in `crates/core/src/lsp/` — Content-Length transport, JSON-RPC envelopes, the bounded protocol-struct set, position mapping (1-based char cols ↔ 0-based UTF-16), a full-sync document store, and a blocking server loop — dispatching to a `LanguageService` trait, fully exercised against a crate-private fake service.

**Architecture:** The asm-framework pattern applied to a second framework: core owns every protocol concern (framing, JSON, position encoding, lifecycle, capability advertisement, diagnostic publishing, semantic-token wire packing) behind one trait that speaks the toolchain's currency (`core::diagnostics::Span`/`Pos`, plain strings). Zero `.pmc`/PM-1 knowledge; the fake service proves it (the `test_arch` philosophy). Plan 2 implements the trait for `.pmc`; plan 3 builds the editor shells.

**Tech Stack:** Rust edition 2024; `serde`/`serde_json` (already deps) + `proptest` (dev, already a dep). Hand-rolled LSP 3.17 subset — no `tower-lsp`, no `lsp-server`, no `lsp-types`, no tokio. Design authority: `docs/superpowers/specs/2026-07-07-pmt-lsp-design.md` (Architecture, Runtime model, Error containment, Testing sections).

## Global Constraints

Every task's requirements implicitly include these. Copy the binding ones into each task's reviewer prompt.

- **Zero new dependencies.** `serde`/`serde_json` runtime, `proptest` dev-only. Gates at every commit: `cargo test --workspace` green, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --check`. Commit only on a clean tree.
- **Core carries zero PM-1 and zero `.pmc` knowledge.** Nothing under `core/src/lsp/` names a `.pmc` concept; all tests run against the crate-private `FakeService`. The words "pmc", "pmt", "label", "namespace-block" must not appear in core code or tests (generic LSP terms like `namespace` as a *symbol kind* are fine — that's protocol vocabulary).
- **Hand-rolled error idiom** (house style): `#[derive(Debug, PartialEq, Eq)] enum XxxError` + hand-written `Display` (lowercase messages) + bare `impl std::error::Error`. No thiserror/anyhow.
- **Thin renderer:** the server loop writes protocol frames only to the injected `Write` handle; panic/log lines go to stderr (`eprintln!` is sanctioned here by the spec — the loop's only side channel). Nothing touches real stdio in core.
- **No stale positions:** every conversion helper takes the current document text; nothing caches derived positions.
- **Protocol structs limited to what the advertised capabilities consume** — no dead protocol surface.
- **Conventional commits**, scope `feat(core):` / `test(core):`. **No AI/Claude attribution footers.**
- Module docs cite durable pages: `docs/lsp.md` (written in plan 2) and `docs/cli.md (thin-renderer rule)`. No issue/PR numbers in code or docs.
- Do **NOT** merge or push; the branch is left for the user's review.

## File Structure

- `crates/core/src/lib.rs` — add `pub mod lsp;` (Task 1).
- `crates/core/src/lsp/mod.rs` — module doc, `LanguageService` trait + service-facing types, `FakeService` fixture (Task 6; stub created in Task 1).
- `crates/core/src/lsp/transport.rs` — Content-Length framing (Task 1).
- `crates/core/src/lsp/jsonrpc.rs` — envelopes, ids, error codes (Task 2).
- `crates/core/src/lsp/types.rs` — the protocol structs (Task 3).
- `crates/core/src/lsp/position.rs` — Span↔Range mapping (Task 4), semantic-token packing (Task 6).
- `crates/core/src/lsp/docstore.rs` — uri → {version, text} (Task 5).
- `crates/core/src/lsp/server.rs` — blocking loop, lifecycle, dispatch (Tasks 7–9).

All tests are inline `#[cfg(test)]` modules (house pattern; the `FakeService` fixture is `pub(crate)` + `#[cfg(test)]`, unreachable from `tests/` by design — same as `test_arch`).

---

### Task 1: Module scaffold + transport framing

**Files:**
- Modify: `crates/core/src/lib.rs` (add `pub mod lsp;` to the alphabetical module list)
- Create: `crates/core/src/lsp/mod.rs` (module doc + `pub mod` lines for `transport` only at this point; others added as tasks land)
- Create: `crates/core/src/lsp/transport.rs`

**Interfaces (Produces):**

```rust
//! mod.rs header:
//! Language-agnostic LSP server framework (LSP 3.17 subset): framing,
//! JSON-RPC, protocol structs, position mapping, document store, and the
//! blocking server loop behind the `LanguageService` seam. Carries zero
//! architecture or language knowledge by contract — exercised against a
//! crate-private fake service (docs/lsp.md; docs/cli.md (thin-renderer rule)).
```

```rust
#[derive(Debug, PartialEq, Eq)]
pub enum TransportError {
    /// Header section ended without a Content-Length header.
    MissingContentLength,
    /// Malformed header line or unparseable length value.
    MalformedHeader(String),
    /// Payload bytes were not valid UTF-8.
    InvalidUtf8,
    /// Underlying read/write failure (message text of the io::Error).
    Io(String),
}
// + Display (lowercase: "missing content-length header", …) + impl std::error::Error

/// Reads one framed message. `Ok(None)` = clean EOF before any header byte.
/// Header lines are `Name: value\r\n`; unknown headers (Content-Type, …) are
/// tolerated; the header block ends at an empty line; then exactly
/// Content-Length bytes of UTF-8 payload follow.
pub fn read_message(reader: &mut dyn std::io::BufRead) -> Result<Option<String>, TransportError>;

/// Writes `Content-Length: N\r\n\r\n` + payload, then flushes.
pub fn write_message(writer: &mut dyn std::io::Write, payload: &str) -> Result<(), TransportError>;
```

Implementation notes: read header lines with a byte loop looking for `\r\n` (do NOT use `read_line` — it splits on `\n` alone, fine, but then trim `\r`; either is acceptable as long as tests pass); EOF *mid*-headers or mid-payload is `Io("unexpected eof")`, only EOF before the first byte is `Ok(None)`. `Content-Length` header name matches case-insensitively (be liberal in what you accept).

**Steps (TDD):**
- [ ] Write failing tests in `transport.rs`: (1) `write_message` then `read_message` round-trips a payload with non-ASCII (`"{\"x\":\"привет 😀\"}"` — Content-Length counts *bytes*, assert that explicitly); (2) two messages back-to-back read in sequence; (3) `Content-Type: application/vscode-jsonrpc; charset=utf-8\r\n` before Content-Length is tolerated; (4) header block with no Content-Length → `MissingContentLength`; (5) clean EOF → `Ok(None)`; (6) EOF mid-payload → `Io`; (7) a reader delivering one byte per `read()` call (custom `Read` impl wrapped in `BufReader::with_capacity(4, …)`) still frames correctly — the split/partial-read case.
- [ ] Add the proptest block (house idiom, `proptest! { #[test] … }`): round-trip arbitrary `String` payloads; and `read_message` **never panics on noise** (`proptest::collection::vec(any::<u8>(), 0..256)` fed via `&mut &noise[..]` — must return `Ok`/`Err`, not panic).
- [ ] Run: `cargo test -p mtc-core lsp::transport` — all fail (unimplemented), then implement, then all pass.
- [ ] Full gates: `cargo test --workspace` + clippy + fmt.
- [ ] Commit: `feat(core): lsp transport — content-length framing over BufRead/Write`

---

### Task 2: JSON-RPC envelopes

**Files:**
- Create: `crates/core/src/lsp/jsonrpc.rs`; add `pub mod jsonrpc;` to `lsp/mod.rs`.

**Interfaces (Produces):**

```rust
/// Request/response id — number or string per JSON-RPC 2.0.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum Id { Number(i64), String(String) }

/// One decoded incoming message.
#[derive(Debug, PartialEq)]
pub enum Message {
    Request { id: Id, method: String, params: serde_json::Value },
    Notification { method: String, params: serde_json::Value },
    /// A response to a server-initiated request; the loop drops these.
    Response { id: Option<Id> },
}

#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Not valid JSON at all → respond ParseError with null id.
    Json(String),
    /// Valid JSON but not a JSON-RPC 2.0 message → InvalidRequest.
    Shape(&'static str),
}
// + Display + std::error::Error

pub fn decode(payload: &str) -> Result<Message, DecodeError>;

// Outgoing encoders (each returns the serialized payload string):
pub fn response_ok(id: &Id, result: serde_json::Value) -> String;
pub fn response_err(id: Option<&Id>, code: i64, message: &str) -> String;   // None id → null
pub fn notification(method: &str, params: serde_json::Value) -> String;
pub fn request(id: i64, method: &str, params: serde_json::Value) -> String;

pub mod error_codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    pub const SERVER_NOT_INITIALIZED: i64 = -32002;
}
```

Decode rules: must be a JSON object (arrays/batches → `Shape`); `method`+`id` → Request; `method` only → Notification; `id` (or `result`/`error`) without `method` → Response; absent `params` decodes as `Value::Null`. Don't validate the `jsonrpc: "2.0"` field strictly (clients always send it; rejecting on it buys nothing).

**Steps (TDD):**
- [ ] Failing tests: decode a request with a **number id** and with a **string id**; a notification; a response (result and error flavors both map to `Message::Response`); missing `params` → `Value::Null`; malformed JSON → `DecodeError::Json`; a JSON array → `DecodeError::Shape`; `response_ok`/`response_err`/`notification`/`request` emit expected JSON (parse the output back with `serde_json::from_str::<Value>` and compare against a hand-written `serde_json::json!` value — never string-compare serialized JSON, key order is unspecified); `response_err(None, …)` carries `"id": null`.
- [ ] Implement; tests green; full gates.
- [ ] Commit: `feat(core): lsp json-rpc envelopes, ids, error codes`

---

### Task 3: Protocol structs

**Files:**
- Create: `crates/core/src/lsp/types.rs`; add `pub mod types;` to `lsp/mod.rs`.

**Interfaces (Produces):** the bounded LSP 3.17 subset the advertised capabilities consume. Common derive block for ALL types: `#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]` + `#[serde(rename_all = "camelCase")]`; every `Option` field gets `#[serde(skip_serializing_if = "Option::is_none")]` and `#[serde(default)]`. Core types written out here; the rest follow mechanically:

```rust
pub struct Position { pub line: u32, pub character: u32 }        // 0-based, UTF-16 cols
pub struct Range { pub start: Position, pub end: Position }      // half-open
pub struct Location { pub uri: String, pub range: Range }
pub struct TextEdit { pub range: Range, pub new_text: String }   // newText on the wire
pub struct WorkspaceEdit {
    pub changes: std::collections::HashMap<String, Vec<TextEdit>>,
}
pub struct WireDiagnostic {                                       // named to avoid clashing with diagnostics::Diagnostic
    pub range: Range,
    pub severity: Option<u32>,          // 1 = Error, 2 = Warning
    pub code: Option<String>,
    pub source: Option<String>,
    pub message: String,
}
pub struct ServerCapabilities {
    pub position_encoding: String,                        // "utf-16"
    pub text_document_sync: TextDocumentSyncOptions,      // { openClose: true, change: 1 }
    pub completion_provider: CompletionOptions,           // { triggerCharacters }
    pub definition_provider: bool,
    pub document_formatting_provider: bool,
    pub document_symbol_provider: bool,
    pub code_action_provider: CodeActionOptions,          // { codeActionKinds: ["quickfix"] }
    pub semantic_tokens_provider: SemanticTokensOptions,  // { legend, full: true }
}
pub struct SemanticTokensLegend {
    pub token_types: Vec<String>,
    pub token_modifiers: Vec<String>,
}
pub struct DocumentSymbol {
    pub name: String,
    pub kind: u32,                       // symbol_kind constants below
    pub range: Range,
    pub selection_range: Range,
    pub children: Vec<DocumentSymbol>,
}
pub struct CompletionItem {
    pub label: String,
    pub kind: Option<u32>,               // completion_item_kind constants below
    pub text_edit: Option<TextEdit>,
}
pub struct CodeAction {
    pub title: String,
    pub kind: String,                    // "quickfix"
    pub is_preferred: Option<bool>,
    pub edit: WorkspaceEdit,
}
```

The remaining types, exact fields (same derives; params structs only need the fields we read — serde ignores unknown JSON keys on deserialize by default, which is the LSP-correct posture):

| type | fields |
|---|---|
| `TextDocumentIdentifier` | `uri: String` |
| `VersionedTextDocumentIdentifier` | `uri: String`, `version: i32` |
| `TextDocumentItem` | `uri: String`, `language_id: String`, `version: i32`, `text: String` |
| `TextDocumentPositionParams` | `text_document: TextDocumentIdentifier`, `position: Position` |
| `DidOpenTextDocumentParams` | `text_document: TextDocumentItem` |
| `DidChangeTextDocumentParams` | `text_document: VersionedTextDocumentIdentifier`, `content_changes: Vec<TextDocumentContentChangeEvent>` |
| `TextDocumentContentChangeEvent` | `text: String` (full sync — no range field) |
| `DidCloseTextDocumentParams` | `text_document: TextDocumentIdentifier` |
| `PublishDiagnosticsParams` | `uri: String`, `version: Option<i32>`, `diagnostics: Vec<WireDiagnostic>` |
| `InitializeParams` | `initialization_options: Option<serde_json::Value>` (client capabilities deliberately unread — we never branch on them in v1) |
| `InitializeResult` | `capabilities: ServerCapabilities`, `server_info: ServerInfoWire` |
| `ServerInfoWire` | `name: String`, `version: String` |
| `TextDocumentSyncOptions` | `open_close: bool`, `change: u32` |
| `CompletionOptions` | `trigger_characters: Vec<String>` |
| `CodeActionOptions` | `code_action_kinds: Vec<String>` |
| `SemanticTokensOptions` | `legend: SemanticTokensLegend`, `full: bool` |
| `CodeActionParams` | `text_document: TextDocumentIdentifier`, `range: Range` |
| `DocumentSymbolParams` | `text_document: TextDocumentIdentifier` |
| `SemanticTokensParams` | `text_document: TextDocumentIdentifier` |
| `SemanticTokens` | `data: Vec<u32>` |
| `DocumentFormattingParams` | `text_document: TextDocumentIdentifier` |
| `DidChangeConfigurationParams` | `settings: serde_json::Value` |
| `DidChangeWatchedFilesParams` | `changes: Vec<FileEvent>` |
| `FileEvent` | `uri: String`, `typ: u32` — field is `"type"` on the wire: `#[serde(rename = "type")]` |
| `Registration` | `id: String`, `method: String`, `register_options: serde_json::Value` |
| `RegistrationParams` | `registrations: Vec<Registration>` |
| `FileSystemWatcher` | `glob_pattern: String` |
| `DidChangeWatchedFilesRegistrationOptions` | `watchers: Vec<FileSystemWatcher>` |

Constants modules (numbers from the LSP 3.17 spec):

```rust
pub mod diagnostic_severity { pub const ERROR: u32 = 1; pub const WARNING: u32 = 2; }
pub mod completion_item_kind {
    pub const FUNCTION: u32 = 3; pub const MODULE: u32 = 9;
    pub const VALUE: u32 = 12; pub const KEYWORD: u32 = 14;
}
pub mod symbol_kind { pub const NAMESPACE: u32 = 3; pub const FUNCTION: u32 = 12; }
```

**Steps (TDD):**
- [ ] Failing tests: (1) `ServerCapabilities` for a sample legend serializes to the exact expected `json!` value (camelCase keys: `positionEncoding`, `textDocumentSync: {"openClose":true,"change":1}`, `completionProvider`, `codeActionProvider: {"codeActionKinds":["quickfix"]}`, `semanticTokensProvider: {"legend":{"tokenTypes":…,"tokenModifiers":…},"full":true}`) — this test IS the capability contract; (2) `DidChangeTextDocumentParams` deserializes from a real client-shaped JSON blob (with extra unknown fields present — must be ignored); (3) `FileEvent` round-trips `"type"`; (4) `WireDiagnostic` with `None` severity omits the key entirely; (5) `TextEdit` serializes `newText`.
- [ ] Implement; green; full gates.
- [ ] Commit: `feat(core): lsp protocol structs (3.17 subset for the advertised capabilities)`

---

### Task 4: Position mapping

**Files:**
- Create: `crates/core/src/lsp/position.rs`; add `pub mod position;` to `lsp/mod.rs`.

**Interfaces:**
- Consumes: `crate::diagnostics::{Pos, Span}` (1-based, char-counted, half-open), `types::{Position, Range}`.
- Produces:

```rust
/// Pos (1-based line, 1-based char col) → Position (0-based line, UTF-16 col),
/// against the current text. Out-of-range input clamps (per LSP): col past
/// end-of-line → line end; line past end-of-file → one past the last line's end.
pub fn pos_to_lsp(text: &str, pos: Pos) -> Position;
/// Inverse; clamps the same way. UTF-16 offsets landing inside a surrogate
/// pair snap to the character start.
pub fn pos_from_lsp(text: &str, position: Position) -> Pos;
pub fn span_to_range(text: &str, span: Span) -> Range;
pub fn range_to_span(text: &str, range: Range) -> Span;
```

Implementation: lines are `text.split('\n')` with any trailing `'\r'` excluded from the countable content (`.pmc` is LF; tolerate CRLF). Walk the line's `chars()`, accumulating `ch.len_utf16()`. No byte arithmetic anywhere — bytes never enter the math.

**Steps (TDD):**
- [ ] Failing tests with hand-computed vectors: ASCII (`"abc\ndef"`: `Pos{2,2}` ↔ `Position{1,1}`); **Cyrillic** (`"привет x"`: `Pos{1,8}` — the `x` — ↔ `Position{0,7}`, guarding against byte-counting: `п` is 2 bytes but 1 char = 1 UTF-16 unit); **astral emoji** (`"😀x"`: `Pos{1,2}` — the `x` — ↔ `Position{0,2}` because `😀` is one char = 2 UTF-16 units); clamping (col 99 on a 3-char line; line 99 in a 2-line file; both directions); a UTF-16 offset landing mid-surrogate snaps to the char start; `span_to_range` maps both endpoints (half-open in, half-open out).
- [ ] Proptest: for strings from the strategy `"[a-zа-я😀\n]{0,40}"` and every valid char position in them, `pos_from_lsp(text, pos_to_lsp(text, p)) == p` (build valid `Pos` values by enumerating lines/chars, not by generating arbitrary numbers).
- [ ] Implement; green; full gates.
- [ ] Commit: `feat(core): lsp position mapping — 1-based char cols to 0-based utf-16, clamping`

---

### Task 5: Document store

**Files:**
- Create: `crates/core/src/lsp/docstore.rs`; add `pub mod docstore;` to `lsp/mod.rs`.

**Interfaces (Produces):**

```rust
pub struct Document { pub version: i32, pub text: String }

#[derive(Default)]
pub struct DocStore { docs: std::collections::HashMap<String, Document> }

impl DocStore {
    pub fn new() -> Self;
    pub fn open(&mut self, uri: &str, version: i32, text: String);
    pub fn change(&mut self, uri: &str, version: i32, text: String); // full-sync replace; no-op if unknown uri
    pub fn close(&mut self, uri: &str);
    pub fn get(&self, uri: &str) -> Option<&Document>;
    pub fn uris(&self) -> Vec<String>;   // sorted, for deterministic republish sweeps
}
```

**Steps (TDD):**
- [ ] Failing tests: open/get; change replaces text + version; close removes; `uris()` sorted; change on unknown uri is a no-op (defensive — a client bug must not panic the server).
- [ ] Implement; green; full gates.
- [ ] Commit: `feat(core): lsp document store — full-sync uri -> version + text`

---

### Task 6: The `LanguageService` seam + semantic-token packing + fake service

**Files:**
- Modify: `crates/core/src/lsp/mod.rs` (trait + service-facing types + `FakeService` fixture)
- Modify: `crates/core/src/lsp/position.rs` (add `pack_semantic_tokens`)

**Interfaces (Produces) — the frozen trait (spec "The LanguageService seam"):**

```rust
use crate::diagnostics::{Edit, Pos, Span};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServiceSeverity { Error, Warning }

/// A diagnostic as the service speaks it: toolchain span + presentation
/// (severity/source/code are presentation, chosen by the service —
/// core::diagnostics::Diagnostic has neither).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServiceDiagnostic {
    pub span: Span,
    pub severity: ServiceSeverity,
    pub source: &'static str,
    pub code: Option<&'static str>,
    pub message: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CandidateKind { Function, Module, Keyword, Value }

/// One completion candidate; inserts via textEdit over the exact token
/// prefix (`replace_span`) so replacement never depends on client-side
/// word heuristics. A zero-width span at the cursor means plain insert.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub label: String,
    pub kind: CandidateKind,
    pub replace_span: Span,
    pub insert_text: String,
}

/// Definition target; `uri` may name a document other than the requester
/// (e.g. a materialized library file on disk).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DefTarget { pub uri: String, pub span: Span }

/// A quickfix: edits apply to the requesting document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Action { pub title: String, pub preferred: bool, pub edits: Vec<Edit> }

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SymbolNodeKind { Namespace, Function }

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SymbolNode {
    pub name: String,
    pub kind: SymbolNodeKind,
    pub span: Span,              // full extent
    pub selection_span: Span,    // the declaration name
    pub children: Vec<SymbolNode>,
}

/// One absolute semantic token. `span` MUST be single-line (contract;
/// the packer debug_asserts it). `token_type` indexes the legend's types;
/// `modifiers` is a bitset over the legend's modifiers.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemToken { pub span: Span, pub token_type: u32, pub modifiers: u32 }

pub trait LanguageService {
    fn language_id(&self) -> &'static str;
    fn trigger_characters(&self) -> &[char];
    /// (token types, token modifiers) — the legend advertised in capabilities.
    fn token_legend(&self) -> (&'static [&'static str], &'static [&'static str]);
    /// Client file-watch globs (e.g. a project config file). May be empty.
    fn watched_globs(&self) -> &'static [&'static str];
    /// Called on didOpen and didChange (framework owns the text); the return
    /// is published as the document's complete diagnostic set. Also re-run by
    /// the framework for every open document after a config or watched-file
    /// change.
    fn did_update(&mut self, uri: &str, text: &str) -> Vec<ServiceDiagnostic>;
    fn did_close(&mut self, uri: &str);
    /// Opaque settings JSON: initializationOptions at startup and
    /// workspace/didChangeConfiguration payloads live.
    fn did_change_config(&mut self, settings: serde_json::Value);
    fn completion(&mut self, uri: &str, pos: Pos) -> Vec<Candidate>;
    fn definition(&mut self, uri: &str, pos: Pos) -> Option<DefTarget>;
    fn code_actions(&mut self, uri: &str, span: Span) -> Vec<Action>;
    fn document_symbols(&mut self, uri: &str) -> Option<Vec<SymbolNode>>;
    fn semantic_tokens(&mut self, uri: &str) -> Option<Vec<SemToken>>;
    /// Full replacement text, or None (degraded / nothing to format against).
    fn format(&mut self, uri: &str) -> Option<String>;
}
```

**Packer** (in `position.rs`):

```rust
/// Absolute tokens → the wire's relative-packed data array
/// (deltaLine, deltaStartChar, length, tokenType, tokenModifiers)×N,
/// sorted by span start; columns and lengths in UTF-16 code units.
pub fn pack_semantic_tokens(text: &str, tokens: &[SemToken]) -> Vec<u32>;
```

**FakeService** (`#[cfg(test)] pub(crate) mod fake` in `mod.rs`): a deterministic toy service used by Tasks 7–9. Behavior spec (implement exactly — the scripted sessions assert against it):
- `language_id` `"fake"`; `trigger_characters` `['.']`; `token_legend` `(["function"], ["declaration"])`; `watched_globs` `["**/fake.json"]`.
- Holds `texts: HashMap<String, String>` and `config_revision: u32` (bumped by every `did_change_config`).
- `did_update`: stores the text; returns one Error `ServiceDiagnostic` (source `"fake"`, code `Some("bad-word")`) spanning each occurrence of the word `bad` (char-precise span), with message `format!("bad word (config rev {})", self.config_revision)` — the embedded revision makes config-triggered republishes observable. **Panics** (`panic!("fake service panic")`) if the text contains `panic-now` — the containment probe.
- `did_close`: removes the stored text.
- `completion`: one candidate `alpha` (Function, zero-width span at the cursor, insert `alpha`).
- `definition`: if the stored text contains `def`, targets the same uri at the span of its first occurrence; else None.
- `code_actions`: for every stored `bad` span overlapping the request span, an `Action { title: "remove bad", preferred: true, edits: [Edit { span, replacement: String::new() }] }`.
- `document_symbols`: `Some([SymbolNode { name: "root", kind: Function, span/selection_span = whole first line }])` when text is non-empty, else `Some(vec![])`.
- `semantic_tokens`: a `(function, declaration)` token per occurrence of `fn`.
- `format`: `Some(text.replace('\t', "    "))` — changed only when tabs are present (both formatting branches testable).

**Steps (TDD):**
- [ ] Failing packer tests with hand-computed vectors (spec Testing): two tokens on one line (`deltaLine` 0, `deltaStart` relative), a token on a later line (`deltaStart` absolute again), a token after an emoji (UTF-16 start/length differ from char counts), unsorted input comes out sorted, empty input → empty vec.
- [ ] Implement trait + types + packer + `FakeService`; add unit tests pinning `FakeService`'s behavior (diagnostic spans of `bad bad`, the panic probe under `catch_unwind`, format changed/unchanged).
- [ ] Green; full gates.
- [ ] Commit: `feat(core): LanguageService seam, service-facing types, semantic-token packing, fake service`

---

### Task 7: Server loop — lifecycle skeleton

**Files:**
- Create: `crates/core/src/lsp/server.rs`; add `pub mod server;` to `lsp/mod.rs`.

**Interfaces (Produces):**

```rust
/// Deployment identity for the initialize handshake (`serverInfo`);
/// the caller (a CLI) supplies it — core knows no product names.
#[derive(Debug, Clone, Copy)]
pub struct ServerIdentity { pub name: &'static str, pub version: &'static str }

/// Blocking server loop; returns the process exit code per the LSP
/// lifecycle (0 after shutdown→exit, 1 on exit without shutdown).
pub fn run(
    reader: &mut dyn std::io::BufRead,
    writer: &mut dyn std::io::Write,
    service: &mut dyn LanguageService,
    identity: ServerIdentity,
) -> i32;
```

Lifecycle rules (spec "Runtime model"), all implemented in this task:
- Requests before `initialize` → error `SERVER_NOT_INITIALIZED` (-32002). Notifications before `initialize` are dropped — except `exit`.
- `initialize`: feed `initialization_options` (when present) to `service.did_change_config`; respond `InitializeResult` built from the service's trigger characters + legend + `identity` (capabilities exactly as pinned in Task 3's test). A second `initialize` → `INVALID_REQUEST`.
- `initialized` (notification): send the `client/registerCapability` request (id from a monotonically increasing server-side counter starting at 1, method `workspace/didChangeWatchedFiles`, watchers from `service.watched_globs()`); skip entirely when the globs are empty.
- `shutdown` → respond `null`, set the flag. `exit` → break the loop; return 0 if shutdown was seen, else 1. EOF on the reader → treat as `exit` without shutdown (return 1) — a dead client must not hang the process.
- Unknown request → `METHOD_NOT_FOUND`; unknown notification (including all `$/…`, e.g. `$/cancelRequest`) → silently dropped.
- Transport `Ok(None)`/malformed JSON → `ParseError` response with null id (malformed) or clean loop end (EOF). `DecodeError::Shape` → `INVALID_REQUEST` with null id.
- Incoming `Message::Response` → dropped silently (the client answering our registerCapability).

Internal shape: a `struct ServerState { initialized: bool, shutdown: bool, next_request_id: i64, docs: DocStore }` and one `fn dispatch(...)` the loop calls per message; the scripted-session test helper lives here too:

```rust
#[cfg(test)]
fn run_session(client_messages: &[serde_json::Value], service: &mut dyn LanguageService)
    -> (Vec<serde_json::Value>, i32)
// frames the inputs into a byte buffer, runs `run` over in-memory pipes
// (&mut &input[..] / Vec<u8>), reads back every output frame, returns
// (decoded outputs in order, exit code).
```

**Steps (TDD):**
- [ ] Failing scripted-session tests (using `run_session` + `FakeService`): (1) `initialize` (with `initializationOptions: {"n":1}`) → response has `capabilities` matching the Task 3 pinned value + `serverInfo {name, version}` from a test identity, and the fake's `config_revision` bumped; (2) `initialized` → a `client/registerCapability` request appears with the `**/fake.json` watcher; (3) a request sent *before* initialize → error -32002 and the session still serves a subsequent initialize; (4) `shutdown` → `null` result, then `exit` → exit code 0; (5) `exit` without shutdown → 1; (6) EOF with no exit → 1; (7) unknown request method after init → -32601; (8) `$/cancelRequest` notification → no output at all; (9) a malformed-JSON frame → ParseError with null id, session continues.
- [ ] Implement; green; full gates.
- [ ] Commit: `feat(core): lsp server loop — lifecycle, dispatch skeleton, scripted-session harness`

---

### Task 8: Server loop — document sync + diagnostics publishing

**Files:**
- Modify: `crates/core/src/lsp/server.rs`.

**Interfaces:**
- Consumes: `DocStore` (Task 5), `ServiceDiagnostic` → `WireDiagnostic` conversion via `position::span_to_range` against the document's current text.
- Produces: handler arms for `textDocument/didOpen`, `didChange`, `didClose`; a private `fn publish(state, writer, uri)` used by every path that must (re)publish.

Rules: `didOpen` stores `{version, text}`, calls `service.did_update`, publishes the full set with the document version. `didChange` takes the **last** element of `content_changes` (full sync) — same flow. `didClose` removes the doc, calls `service.did_close`, publishes an **empty** set (version omitted). Severity mapping: `Error → 1`, `Warning → 2`; `code`/`source` copied; spans converted against the *new* text (no stale positions).

**Steps (TDD):**
- [ ] Failing scripted-session tests: (1) initialize → didOpen (`"ok bad ok"`) → a `textDocument/publishDiagnostics` notification with one diagnostic, correct char-precise range, `severity: 1`, `source: "fake"`, `code: "bad-word"`, `version: 1`; (2) didChange to `"all clear"` (version 2) → publish with empty `diagnostics` and `version: 2`; (3) didChange to `"bad"` then didClose → final publish is empty; (4) two `bad`s → two diagnostics, source-ordered.
- [ ] Implement; green; full gates.
- [ ] Commit: `feat(core): lsp document sync + publishDiagnostics`

---

### Task 9: Server loop — feature dispatch, config, watched files, panic containment

**Files:**
- Modify: `crates/core/src/lsp/server.rs`.

**Interfaces:** handler arms for the six feature requests plus the two workspace notifications, each converting trait output to protocol types:

- `textDocument/completion`: `pos_from_lsp` the position → `service.completion` → `Vec<CompletionItem>` (kind per `CandidateKind` → `completion_item_kind` constants; `text_edit` from `replace_span` + `insert_text`). Result: the JSON array (a bare `CompletionItem[]` is a legal completion result — no `CompletionList` wrapper needed).
- `textDocument/definition`: → `Option<DefTarget>` → `Location` or `null`. **Range conversion:** when the target uri is open in the DocStore, convert against its text; otherwise convert with the char==UTF-16 identity (subtract 1 only). Document this contract on `DefTarget`: external targets are exact when the target's lines are ASCII up to the span — true by construction for the materialized stdlib (plan 2 guards it with a test).
- `textDocument/codeAction`: `range_to_span` → `service.code_actions` → `CodeAction[]` with `kind: "quickfix"`, `is_preferred`, `edit.changes = { uri: edits→TextEdits }`.
- `textDocument/documentSymbol`: → `Option<Vec<SymbolNode>>` → recursive `DocumentSymbol[]` or `null`.
- `textDocument/semanticTokens/full`: → `Option<Vec<SemToken>>` → `SemanticTokens { data: pack_semantic_tokens(text, &toks) }` or `null`.
- `textDocument/formatting`: → `service.format`. `None` → `null`; `Some(t)` equal to the stored text → `[]`; else one whole-document `TextEdit` (range from `Position{0,0}` to the end-of-text position computed by clamping `Pos{u32::MAX, u32::MAX}`).
- `workspace/didChangeConfiguration`: `service.did_change_config(params.settings)`, then re-run `did_update` + publish for **every** open document (sorted uris).
- `workspace/didChangeWatchedFiles`: no config parsing in core — just the same republish-all sweep (the service re-reads its own config sources during `did_update`).
- **Panic containment:** wrap each dispatch in `std::panic::catch_unwind(AssertUnwindSafe(…))`. A panicking *request* handler → `INTERNAL_ERROR` response carrying the panic payload text; a panicking *notification* handler → stderr line only. The loop continues either way. (Set a no-op panic hook around the call — `std::panic::set_hook`/`take_hook` — so the default hook doesn't spam stderr with backtraces in tests; re-emit one concise `eprintln!` line instead.)

**Steps (TDD):**
- [ ] Failing scripted-session tests, one per feature against `FakeService`: completion (textEdit shape + kind 3), definition (in-file hit + null miss), codeAction (overlapping span yields the quickfix with `isPreferred: true` and the delete edit), documentSymbol (root node, kind 12), semanticTokens (packed data for two `fn` occurrences — hand-computed), formatting (tabbed text → one whole-doc edit; tab-free → `[]`).
- [ ] Failing test — config replumb: open TWO docs, send `didChangeConfiguration` → exactly two publishes whose messages embed the bumped config revision (this asserts the framework re-publishes ALL open docs).
- [ ] Failing test — watched files: `didChangeWatchedFiles` → same two republishes.
- [ ] Failing test — containment: didOpen a doc containing `panic-now` → no publish for it, a subsequent `initialize`-already-done request (e.g. completion on a healthy doc) still answers correctly; and a *request* that panics (point completion at a `panic-now` doc — extend `FakeService::completion` to panic when the stored text contains `panic-now`) → `-32603` response, session alive after.
- [ ] Implement; green; full gates.
- [ ] The full spec-shaped session as one final test: initialize → didOpen → publish → completion/definition/formatting → didChange → didChangeConfiguration (republish all) → didChangeWatchedFiles (republish all) → shutdown → exit, asserting exit code 0 (spec Testing, core bullet 5).
- [ ] Commit: `feat(core): lsp feature dispatch, config + watched-file replumb, panic containment`

---

## Self-check before handoff to plan 2

- `cargo test -p mtc-core` — every lsp test green; grep `crates/core/src/lsp` for `pmc`/`pmt `/`PM-1` — only the sanctioned doc-page citations (`docs/lsp.md`, `docs/cli.md`) may appear.
- The trait + service types compiled exactly as frozen here — plan 2's `PmcLanguageService` implements this signature verbatim; any drift discovered while implementing plan 2 is fixed by amending plan 2, not silently changing the trait.
- All three gates green at the final commit.
