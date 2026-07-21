//! Blocking LSP server loop (docs/lsp.md "Runtime model"): reads
//! Content-Length-framed JSON-RPC messages off `reader`, dispatches them
//! against one or more [`LanguageService`]s, and enforces the LSP
//! lifecycle — initialize/initialized/shutdown/exit gating,
//! unknown-method handling, and decode-error responses. When several
//! services share one stdio endpoint (docs/lsp.md, multi-service
//! routing), initialize merges their capabilities and each document
//! binds to exactly one service on didOpen so later messages route
//! deterministically; a lone service degenerates to a dedicated server.
//! Document sync (didOpen/didChange/didClose) drives the `DocStore` and
//! republishes diagnostics through one `publish` helper; the same helper
//! powers the config- and watched-file-triggered republish-all sweeps.
//! Feature requests (completion, definition, code actions, document
//! symbols, semantic tokens, formatting) convert the bound service's
//! output to wire types via `position`. Every dispatched message runs
//! under `catch_unwind` so a panicking handler can't take the whole
//! session down (docs/lsp.md "Error containment").

use std::cell::Cell;
use std::collections::HashMap;
use std::panic::{self, AssertUnwindSafe};
use std::sync::Once;

use super::LanguageService;
use super::docstore::DocStore;
use super::jsonrpc::{self, DecodeError, Id, Message, error_codes};
use super::position;
use super::transport;
use super::types::{
    CodeAction, CodeActionOptions, CodeActionParams, CompletionItem, CompletionOptions,
    DidChangeConfigurationParams, DidChangeTextDocumentParams, DidChangeWatchedFilesParams,
    DidChangeWatchedFilesRegistrationOptions, DidCloseTextDocumentParams,
    DidOpenTextDocumentParams, DocumentFormattingParams, DocumentSymbol, DocumentSymbolParams,
    FileSystemWatcher, Hover, InitializeParams, InitializeResult, Location, LocationLink,
    MarkupContent, Position, PublishDiagnosticsParams, Range, Registration, RegistrationParams,
    SemanticTokens, SemanticTokensLegend, SemanticTokensOptions, SemanticTokensParams,
    ServerCapabilities, ServerInfoWire, TextDocumentPositionParams, TextDocumentSyncOptions,
    TextEdit, WireDiagnostic, WorkspaceEdit, completion_item_kind, completion_item_tag,
    diagnostic_severity, diagnostic_tag, symbol_kind,
};
use super::{
    Action, Candidate, CandidateKind, DefTarget, HoverContent, SemToken, ServiceDiagnostic,
    ServiceSeverity, SymbolNode, SymbolNodeKind,
};
use crate::diagnostics::{Pos, Span};

/// Deployment identity for the initialize handshake (`serverInfo`); the
/// caller (a CLI) supplies it — core knows no product names.
#[derive(Debug, Clone, Copy)]
pub struct ServerIdentity {
    pub name: &'static str,
    pub version: &'static str,
}

/// Mutable state threaded through `dispatch` across the whole session.
struct ServerState {
    initialized: bool,
    shutdown: bool,
    next_request_id: i64,
    docs: DocStore,
    /// `params.capabilities.textDocument.definition.linkSupport` from
    /// `initialize`, read via a raw JSON pointer walk — the one client
    /// capability this server reads. Gates whether `textDocument/definition`
    /// may answer with `LocationLink` (sending one to a non-declaring
    /// client is a protocol violation).
    definition_link_support: bool,
    /// Which service (by index into `services`) owns each open URI
    /// (docs/lsp.md, multi-service routing). Bound on didOpen by
    /// languageId → extension → service 0; every later request or
    /// notification on that URI routes through the binding; didClose
    /// removes it. An unbound URI routes to service 0 (`unwrap_or(0)`) —
    /// which reproduces the single-service "not-open" behavior exactly,
    /// since a lone service is always index 0.
    bindings: HashMap<String, usize>,
    /// Per-service semantic-token-legend remap tables, built once at
    /// initialize from the merged legend (docs/lsp.md, capability merge).
    /// `type_offsets[i]` is service `i`'s base index in the concatenated
    /// `tokenTypes`; `modifier_maps[i][b]` is the merged bit that service
    /// `i`'s local modifier bit `b` relocates to in the dedup-union
    /// `tokenModifiers`. For a single service both are identities
    /// (`type_offsets == [0]`, `modifier_maps == [[0, 1, …]]`), so wire
    /// output stays byte-identical.
    type_offsets: Vec<u32>,
    modifier_maps: Vec<Vec<u32>>,
}

impl ServerState {
    fn new() -> Self {
        ServerState {
            initialized: false,
            shutdown: false,
            next_request_id: 1,
            docs: DocStore::new(),
            definition_link_support: false,
            bindings: HashMap::new(),
            type_offsets: Vec::new(),
            modifier_maps: Vec::new(),
        }
    }
}

/// What the loop does after a dispatched message has been handled.
enum Signal {
    Continue,
    Exit,
}

/// Blocking server loop; returns the process exit code per the LSP
/// lifecycle (0 after shutdown→exit, 1 on exit without shutdown, and 1
/// on EOF/transport failure — a dead client must not hang the process).
///
/// Serves one or more [`LanguageService`]s behind a single stdio endpoint
/// (docs/lsp.md, multi-service routing): the initialize handshake merges
/// their capabilities, and every document is bound to exactly one service
/// on didOpen so later requests route deterministically. A single-service
/// call (`&mut [&mut service]`) is behaviorally identical to a dedicated
/// server — the merge maps degenerate to identities.
pub fn run(
    reader: &mut dyn std::io::BufRead,
    writer: &mut dyn std::io::Write,
    services: &mut [&mut dyn LanguageService],
    identity: ServerIdentity,
) -> i32 {
    let mut state = ServerState::new();
    install_quiet_hook_once();

    loop {
        let payload = match transport::read_message(reader) {
            Ok(Some(payload)) => payload,
            // Clean EOF or a transport-level failure: both mean the
            // client is gone. Treat exactly like `exit` without
            // `shutdown` — the process must not hang waiting for bytes
            // that will never arrive.
            Ok(None) | Err(_) => return 1,
        };

        let message = match jsonrpc::decode(&payload) {
            Ok(message) => message,
            Err(DecodeError::Json(msg)) => {
                let response = jsonrpc::response_err(None, error_codes::PARSE_ERROR, &msg);
                let _ = transport::write_message(writer, &response);
                continue;
            }
            Err(DecodeError::Shape(reason)) => {
                let response = jsonrpc::response_err(None, error_codes::INVALID_REQUEST, reason);
                let _ = transport::write_message(writer, &response);
                continue;
            }
        };

        match dispatch(&mut state, writer, services, identity, message) {
            Signal::Continue => {}
            Signal::Exit => return if state.shutdown { 0 } else { 1 },
        }
    }
}

/// Routes one decoded message to the request/notification handlers;
/// `Message::Response`s (the client answering a server-initiated
/// request such as `client/registerCapability`) are dropped silently.
///
/// **Panic containment** (docs/lsp.md "Error containment"): the routed
/// call runs under `catch_unwind`, so one bad handler can't take the
/// session down. A panicking *request* handler answers with
/// `INTERNAL_ERROR` carrying the panic payload text (no response can
/// have been written yet — every handler either panics or writes
/// exactly one response, never both); a panicking *notification*
/// handler produces no output at all beyond a concise stderr line. The
/// loop always continues either way (`run` decides EOF-flavored exits;
/// a contained panic is not one of them). Each `catch_unwind` is wrapped
/// in a `SuppressPanicOutput` guard so the process-global delegating
/// hook (`install_quiet_hook_once`) swallows the default panic
/// diagnostic for exactly this thread, exactly for this one dispatched
/// message — never a wider window, never another thread.
fn dispatch(
    state: &mut ServerState,
    writer: &mut dyn std::io::Write,
    services: &mut [&mut dyn LanguageService],
    identity: ServerIdentity,
    message: Message,
) -> Signal {
    match message {
        Message::Request { id, method, params } => {
            let outcome = {
                let _suppress = SuppressPanicOutput::new();
                panic::catch_unwind(AssertUnwindSafe(|| {
                    handle_request(state, writer, services, identity, &id, &method, params);
                }))
            };
            if let Err(payload) = outcome {
                let text = panic_message(&*payload);
                eprintln!("lsp server: request '{method}' panicked: {text}");
                respond_err(writer, Some(&id), error_codes::INTERNAL_ERROR, &text);
            }
            Signal::Continue
        }
        Message::Notification { method, params } => {
            let outcome = {
                let _suppress = SuppressPanicOutput::new();
                panic::catch_unwind(AssertUnwindSafe(|| {
                    handle_notification(state, writer, services, &method, params)
                }))
            };
            match outcome {
                Ok(signal) => signal,
                Err(payload) => {
                    let text = panic_message(&*payload);
                    eprintln!("lsp server: notification '{method}' panicked: {text}");
                    Signal::Continue
                }
            }
        }
        Message::Response { .. } => Signal::Continue,
    }
}

/// Best-effort text extraction from a caught panic payload: `panic!`
/// with a string literal or a `String` message (the vast majority of
/// panics, including every panic this crate's own code raises)
/// downcasts cleanly; anything else renders as a fixed placeholder
/// rather than losing the response entirely.
fn panic_message(payload: &(dyn std::any::Any + Send)) -> String {
    if let Some(message) = payload.downcast_ref::<&str>() {
        (*message).to_string()
    } else if let Some(message) = payload.downcast_ref::<String>() {
        message.clone()
    } else {
        "panic payload was not a string".to_string()
    }
}

thread_local! {
    /// Set for the extent of exactly one `catch_unwind` call (see
    /// `SuppressPanicOutput`) so the shared hook's delegate-or-swallow
    /// decision is scoped to a single thread's containment window, never
    /// to a process-wide interval another thread's unrelated panic could
    /// land inside.
    static SUPPRESS_PANIC_OUTPUT: Cell<bool> = const { Cell::new(false) };
}

/// Guards one-time installation of the process-global delegating hook
/// (see `install_quiet_hook_once`) so concurrent callers (parallel
/// `run_session` tests, for instance) can call it freely — the first
/// caller installs, every later call is a no-op.
static HOOK_INIT: Once = Once::new();

/// Installs, once and for the rest of the process's life, a panic hook
/// that delegates to whatever hook was previously registered UNLESS the
/// *panicking* thread currently has `SUPPRESS_PANIC_OUTPUT` set.
///
/// This replaces an earlier design that swapped the process-global hook
/// in and out around each `run()` call (install on entry, restore on
/// drop). That design raced under `cargo test`'s default multi-threaded
/// execution: an unrelated test panicking while a `run_session` test was
/// mid-flight could have its default diagnostic silently swallowed by
/// the installed no-op hook, and two concurrent install/restore
/// interleavings could restore the wrong "previous" hook, permanently
/// leaking a no-op hook for the rest of the suite.
///
/// The delegating hook sidesteps both failure modes by never being
/// installed or removed more than once: it is transparent (delegates to
/// `previous`) to every thread by default, and only the thread currently
/// inside a `dispatch` call's containment window (or the crate's
/// panic-containment fixture test) suppresses output, via a
/// thread-local read that runs on the panicking thread itself — so
/// suppression can never leak onto another thread's panic.
pub(crate) fn install_quiet_hook_once() {
    HOOK_INIT.call_once(|| {
        let previous = panic::take_hook();
        panic::set_hook(Box::new(move |info| {
            if !SUPPRESS_PANIC_OUTPUT.with(Cell::get) {
                previous(info);
            }
        }));
    });
}

/// RAII guard scoping `SUPPRESS_PANIC_OUTPUT` to one `catch_unwind` call:
/// sets the thread-local flag true on construction, clears it on drop.
/// Drop runs on every path out of the enclosing block — panic-caught,
/// clean return, or (via `catch_unwind`'s boundary) even the panicking
/// path itself — so suppression can never leak into the next dispatched
/// message on this thread.
pub(crate) struct SuppressPanicOutput;

impl SuppressPanicOutput {
    pub(crate) fn new() -> Self {
        SUPPRESS_PANIC_OUTPUT.with(|flag| flag.set(true));
        SuppressPanicOutput
    }
}

impl Drop for SuppressPanicOutput {
    fn drop(&mut self) {
        SUPPRESS_PANIC_OUTPUT.with(|flag| flag.set(false));
    }
}

fn handle_request(
    state: &mut ServerState,
    writer: &mut dyn std::io::Write,
    services: &mut [&mut dyn LanguageService],
    identity: ServerIdentity,
    id: &Id,
    method: &str,
    params: serde_json::Value,
) {
    match method {
        "initialize" => handle_initialize(state, writer, services, identity, id, params),
        _ if !state.initialized => {
            respond_err(
                writer,
                Some(id),
                error_codes::SERVER_NOT_INITIALIZED,
                "server not initialized",
            );
        }
        "shutdown" => {
            state.shutdown = true;
            respond_ok(writer, id, serde_json::Value::Null);
        }
        "textDocument/completion" => handle_completion(state, writer, services, id, params),
        "textDocument/definition" => handle_definition(state, writer, services, id, params),
        "textDocument/hover" => handle_hover(state, writer, services, id, params),
        "textDocument/codeAction" => handle_code_action(state, writer, services, id, params),
        "textDocument/documentSymbol" => {
            handle_document_symbol(state, writer, services, id, params)
        }
        "textDocument/semanticTokens/full" => {
            handle_semantic_tokens(state, writer, services, id, params)
        }
        "textDocument/formatting" => handle_formatting(state, writer, services, id, params),
        _ => {
            respond_err(
                writer,
                Some(id),
                error_codes::METHOD_NOT_FOUND,
                &format!("method not found: {method}"),
            );
        }
    }
}

fn handle_initialize(
    state: &mut ServerState,
    writer: &mut dyn std::io::Write,
    services: &mut [&mut dyn LanguageService],
    identity: ServerIdentity,
    id: &Id,
    params: serde_json::Value,
) {
    if state.initialized {
        respond_err(
            writer,
            Some(id),
            error_codes::INVALID_REQUEST,
            "server already initialized",
        );
        return;
    }

    // The one client capability this server reads: everything else
    // under `params.capabilities` stays unparsed and unread. Walked
    // against the raw JSON before `params` is consumed below, since
    // `InitializeParams` itself carries no capabilities field.
    state.definition_link_support = params
        .pointer("/capabilities/textDocument/definition/linkSupport")
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false);

    let init_params: InitializeParams =
        serde_json::from_value(params).unwrap_or(InitializeParams {
            initialization_options: None,
        });
    // initializationOptions reach every service — config is not scoped to
    // a document, so every service must see it (mirrors the
    // didChangeConfiguration broadcast below).
    if let Some(options) = init_params.initialization_options {
        for service in services.iter_mut() {
            service.did_change_config(options.clone());
        }
    }

    state.initialized = true;

    let merged = merge_legend(services);
    let result = build_initialize_result(services, &merged, identity);
    let result_value =
        serde_json::to_value(result).expect("InitializeResult is always serializable");
    // Retain the per-service remap tables for the whole session: every
    // semantic-tokens response relocates the bound service's local indices
    // into the merged legend's index space through these.
    state.type_offsets = merged.type_offsets;
    state.modifier_maps = merged.modifier_maps;
    respond_ok(writer, id, result_value);
}

/// The merged semantic-token legend plus the per-service remap tables
/// (docs/lsp.md, capability merge). `token_types` is the ordered
/// concatenation of every service's types (NOT deduped — see
/// `merge_legend`); `token_modifiers` is their ordered dedup-union.
struct MergedLegend {
    token_types: Vec<String>,
    token_modifiers: Vec<String>,
    /// `type_offsets[i]` = service `i`'s base index in `token_types`.
    type_offsets: Vec<u32>,
    /// `modifier_maps[i][b]` = the merged bit that service `i`'s local
    /// modifier bit `b` maps to.
    modifier_maps: Vec<Vec<u32>>,
}

/// Builds the merged semantic-token legend and the remap tables the
/// server uses to relocate each service's tokens into the merged index
/// space (docs/lsp.md, capability merge).
///
/// The two axes are treated **asymmetrically, deliberately**: token TYPES
/// are concatenated without dedup (a type index is just a legend entry;
/// duplicate names across services are legal LSP and keep the per-service
/// offset a trivial running length), while token MODIFIERS are
/// dedup-unioned (they are bit positions in a 32-bit-wide set — a scarce
/// resource — so two services naming the same modifier must share one
/// bit).
fn merge_legend(services: &[&mut dyn LanguageService]) -> MergedLegend {
    let mut token_types: Vec<String> = Vec::new();
    let mut token_modifiers: Vec<String> = Vec::new();
    let mut type_offsets: Vec<u32> = Vec::with_capacity(services.len());
    let mut modifier_maps: Vec<Vec<u32>> = Vec::with_capacity(services.len());

    for service in services.iter() {
        let (types, modifiers) = service.token_legend();

        // Types: this service's block starts where the concatenation
        // currently ends; then append all of them verbatim.
        type_offsets.push(token_types.len() as u32);
        token_types.extend(types.iter().map(|name| (*name).to_string()));

        // Modifiers: each local bit maps to the merged bit for its name —
        // reusing an existing merged bit when the name is already present.
        let mut map = Vec::with_capacity(modifiers.len());
        for name in modifiers.iter() {
            let merged_bit = match token_modifiers.iter().position(|existing| existing == name) {
                Some(pos) => pos as u32,
                None => {
                    token_modifiers.push((*name).to_string());
                    (token_modifiers.len() - 1) as u32
                }
            };
            map.push(merged_bit);
        }
        modifier_maps.push(map);
    }

    MergedLegend {
        token_types,
        token_modifiers,
        type_offsets,
        modifier_maps,
    }
}

/// Ordered dedup-union of every service's trigger characters, first-seen
/// order preserved (docs/lsp.md, capability merge). One service's set is
/// returned unchanged, keeping the single-service capability byte-identical.
fn merge_trigger_characters(services: &[&mut dyn LanguageService]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for service in services.iter() {
        for character in service.trigger_characters() {
            let text = character.to_string();
            if !out.contains(&text) {
                out.push(text);
            }
        }
    }
    out
}

/// Ordered dedup-union of every service's watched globs, first-seen order
/// preserved (docs/lsp.md, capability merge).
fn merge_watched_globs(services: &[&mut dyn LanguageService]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for service in services.iter() {
        for glob in service.watched_globs() {
            let text = (*glob).to_string();
            if !out.contains(&text) {
                out.push(text);
            }
        }
    }
    out
}

/// Notifications carry no response; the return value is the loop
/// signal (only `exit` produces `Signal::Exit`).
fn handle_notification(
    state: &mut ServerState,
    writer: &mut dyn std::io::Write,
    services: &mut [&mut dyn LanguageService],
    method: &str,
    params: serde_json::Value,
) -> Signal {
    match method {
        "exit" => Signal::Exit,
        // Notifications before initialize are dropped — except `exit`,
        // handled above.
        _ if !state.initialized => Signal::Continue,
        "initialized" => {
            send_register_capability(state, writer, services);
            Signal::Continue
        }
        "textDocument/didOpen" => {
            if let Ok(params) = serde_json::from_value::<DidOpenTextDocumentParams>(params) {
                let uri = params.text_document.uri;
                // Bind the document to a service now; every later message
                // on this URI routes through the binding (docs/lsp.md,
                // multi-service routing).
                let idx = bind_service(services, &uri, &params.text_document.language_id);
                state.bindings.insert(uri.clone(), idx);
                state.docs.open(
                    &uri,
                    params.text_document.version,
                    params.text_document.text,
                );
                publish(state, writer, services, &uri);
            }
            Signal::Continue
        }
        "textDocument/didChange" => {
            if let Ok(mut params) = serde_json::from_value::<DidChangeTextDocumentParams>(params) {
                let uri = params.text_document.uri;
                // Full sync: only the last content-change entry matters.
                if let Some(change) = params.content_changes.pop() {
                    state
                        .docs
                        .change(&uri, params.text_document.version, change.text);
                }
                publish(state, writer, services, &uri);
            }
            Signal::Continue
        }
        "textDocument/didClose" => {
            if let Ok(params) = serde_json::from_value::<DidCloseTextDocumentParams>(params) {
                let uri = params.text_document.uri;
                // Route the close to the bound service BEFORE dropping the
                // binding, then unbind — a later request on this URI is
                // now "not open" and routes to service 0.
                let idx = state.bindings.get(&uri).copied().unwrap_or(0);
                state.docs.close(&uri);
                services[idx].did_close(&uri);
                state.bindings.remove(&uri);
                publish(state, writer, services, &uri);
            }
            Signal::Continue
        }
        "workspace/didChangeConfiguration" => {
            if let Ok(params) = serde_json::from_value::<DidChangeConfigurationParams>(params) {
                // Config is not document-scoped: broadcast to every service,
                // then re-run each open document through its bound service.
                for service in services.iter_mut() {
                    service.did_change_config(params.settings.clone());
                }
                republish_all(state, writer, services);
            }
            Signal::Continue
        }
        // No config parsing here — core stays language-agnostic; the
        // service re-reads its own config/watched sources during
        // `did_update`. This notification exists only to trigger the
        // same republish-all sweep (each open document through its bound
        // service).
        "workspace/didChangeWatchedFiles" => {
            if serde_json::from_value::<DidChangeWatchedFilesParams>(params).is_ok() {
                republish_all(state, writer, services);
            }
            Signal::Continue
        }
        // Unknown notifications (including every `$/…` method, e.g.
        // `$/cancelRequest`) are silently dropped per spec.
        _ => Signal::Continue,
    }
}

/// Chooses the service a freshly opened document binds to (docs/lsp.md,
/// multi-service routing): languageId exact match first, then a
/// case-insensitive URI-suffix match against each service's
/// `extensions()`, finally service 0 with a stderr note. Never a hard
/// error — a wrong-language binding still yields *some* diagnostics,
/// which beats silence, and a lone service always resolves to index 0
/// regardless.
fn bind_service(services: &[&mut dyn LanguageService], uri: &str, language_id: &str) -> usize {
    if let Some(idx) = services
        .iter()
        .position(|service| service.language_id() == language_id)
    {
        return idx;
    }
    // Extensions are ASCII in every registration, so ASCII lowercasing
    // (not full Unicode case-folding) is the correct compare here — it
    // lets `X.TMA`/`X.PMA` etc. bind the same as their lowercase form
    // without touching the registrations themselves.
    let uri_lower = uri.to_ascii_lowercase();
    if let Some(idx) = services.iter().position(|service| {
        service
            .extensions()
            .iter()
            .any(|ext| uri_lower.ends_with(ext.to_ascii_lowercase().as_str()))
    }) {
        return idx;
    }
    // eprintln! is this loop's one sanctioned side channel (docs/lsp.md,
    // multi-service routing): stderr never collides with the stdio
    // protocol stream on stdout.
    eprintln!(
        "lsp server: no service claims languageId '{language_id}' or the extension of '{uri}'; binding to service 0"
    );
    0
}

/// Publishes `uri`'s complete diagnostic set. The single path every
/// (re)publish goes through (didOpen/didChange above; the
/// config/watched-files republish sweeps reuse it once per open
/// document). Looks the document up in the store itself: when open,
/// re-runs the *bound* service's `did_update` against its current text
/// and publishes with its version; when absent (post-didClose),
/// publishes an empty set with the version omitted.
fn publish(
    state: &ServerState,
    writer: &mut dyn std::io::Write,
    services: &mut [&mut dyn LanguageService],
    uri: &str,
) {
    let (version, diagnostics) = match state.docs.get(uri) {
        Some(doc) => {
            let idx = state.bindings.get(uri).copied().unwrap_or(0);
            let diagnostics = services[idx]
                .did_update(uri, &doc.text)
                .iter()
                .map(|diagnostic| to_wire_diagnostic(&doc.text, diagnostic))
                .collect();
            (Some(doc.version), diagnostics)
        }
        None => (None, Vec::new()),
    };

    let params = PublishDiagnosticsParams {
        uri: uri.to_string(),
        version,
        diagnostics,
    };
    let payload = jsonrpc::notification(
        "textDocument/publishDiagnostics",
        serde_json::to_value(params).expect("PublishDiagnosticsParams is always serializable"),
    );
    let _ = transport::write_message(writer, &payload);
}

/// Converts one service-level diagnostic to its wire shape, converting
/// the span against `text` (the document's CURRENT text — no stale
/// positions).
fn to_wire_diagnostic(text: &str, diagnostic: &ServiceDiagnostic) -> WireDiagnostic {
    WireDiagnostic {
        range: position::span_to_range(text, diagnostic.span),
        severity: Some(match diagnostic.severity {
            ServiceSeverity::Error => diagnostic_severity::ERROR,
            ServiceSeverity::Warning => diagnostic_severity::WARNING,
        }),
        code: diagnostic.code.map(|code| code.to_string()),
        source: Some(diagnostic.source.to_string()),
        message: diagnostic.message.clone(),
        tags: diagnostic
            .deprecated
            .then(|| vec![diagnostic_tag::DEPRECATED]),
    }
}

/// Re-publishes diagnostics for every open document, in URI-sorted
/// order — the sweep both `workspace/didChangeConfiguration` and
/// `workspace/didChangeWatchedFiles` trigger. Each document flows through
/// its own bound service (`publish` resolves the binding per URI). Core
/// has no opinion on what changed; the service re-reads its own
/// config/watched-file sources from inside `did_update` itself.
fn republish_all(
    state: &ServerState,
    writer: &mut dyn std::io::Write,
    services: &mut [&mut dyn LanguageService],
) {
    for uri in state.docs.uris() {
        publish(state, writer, services, &uri);
    }
}

/// The current text of an open document, or `""` when `uri` isn't
/// open. Defensive: a feature request against an unopened document
/// converts positions against an empty document rather than
/// panicking (a well-behaved client never does this — every feature
/// request targets a document it has opened).
fn doc_text<'a>(state: &'a ServerState, uri: &str) -> &'a str {
    state
        .docs
        .get(uri)
        .map(|doc| doc.text.as_str())
        .unwrap_or("")
}

fn handle_completion(
    state: &ServerState,
    writer: &mut dyn std::io::Write,
    services: &mut [&mut dyn LanguageService],
    id: &Id,
    params: serde_json::Value,
) {
    let Ok(params) = serde_json::from_value::<TextDocumentPositionParams>(params) else {
        respond_err(
            writer,
            Some(id),
            error_codes::INVALID_PARAMS,
            "invalid completion params",
        );
        return;
    };
    let uri = params.text_document.uri;
    let text = doc_text(state, &uri);
    let pos = position::pos_from_lsp(text, params.position);
    let idx = state.bindings.get(&uri).copied().unwrap_or(0);

    let items: Vec<CompletionItem> = services[idx]
        .completion(&uri, pos)
        .into_iter()
        .map(|candidate| to_completion_item(text, candidate))
        .collect();

    let result = serde_json::to_value(items).expect("CompletionItem[] is always serializable");
    respond_ok(writer, id, result);
}

/// One candidate's insertion is entirely textEdit-driven: the wire
/// item carries no separate insert-text field (docs/lsp.md).
fn to_completion_item(text: &str, candidate: Candidate) -> CompletionItem {
    CompletionItem {
        label: candidate.label,
        kind: Some(match candidate.kind {
            CandidateKind::Function => completion_item_kind::FUNCTION,
            CandidateKind::Module => completion_item_kind::MODULE,
            CandidateKind::Keyword => completion_item_kind::KEYWORD,
            CandidateKind::Value => completion_item_kind::VALUE,
        }),
        detail: candidate.detail,
        tags: candidate
            .deprecated
            .then(|| vec![completion_item_tag::DEPRECATED]),
        text_edit: Some(TextEdit {
            range: position::span_to_range(text, candidate.replace_span),
            new_text: candidate.insert_text,
        }),
    }
}

fn handle_definition(
    state: &ServerState,
    writer: &mut dyn std::io::Write,
    services: &mut [&mut dyn LanguageService],
    id: &Id,
    params: serde_json::Value,
) {
    let Ok(params) = serde_json::from_value::<TextDocumentPositionParams>(params) else {
        respond_err(
            writer,
            Some(id),
            error_codes::INVALID_PARAMS,
            "invalid definition params",
        );
        return;
    };
    let uri = params.text_document.uri;
    let text = doc_text(state, &uri);
    let pos = position::pos_from_lsp(text, params.position);
    let idx = state.bindings.get(&uri).copied().unwrap_or(0);

    let result = match services[idx].definition(&uri, pos) {
        // A LocationLink is only valid to send when the client declared
        // support for it; a target with no origin has nothing to put in
        // originSelectionRange, so it falls back to a plain Location too.
        Some(target) if state.definition_link_support && target.origin.is_some() => {
            serde_json::to_value(vec![to_location_link(state, text, target)])
                .expect("LocationLink[] is always serializable")
        }
        Some(target) => serde_json::to_value(to_location(state, target))
            .expect("Location is always serializable"),
        None => serde_json::Value::Null,
    };
    respond_ok(writer, id, result);
}

/// The wire `Range` for `target`'s span, per `DefTarget`'s documented
/// range-conversion contract: exact against the target's own text when
/// it's open in the DocStore, else the char==UTF-16 identity. Shared by
/// both `to_location` and `to_location_link` — the two response shapes
/// agree on where the target points, differing only in whether an origin
/// range rides along.
fn target_range(state: &ServerState, target: &DefTarget) -> Range {
    match state.docs.get(&target.uri) {
        Some(doc) => position::span_to_range(&doc.text, target.span),
        None => span_to_range_identity(target.span),
    }
}

/// Converts a `DefTarget` to a wire `Location`.
fn to_location(state: &ServerState, target: DefTarget) -> Location {
    let range = target_range(state, &target);
    Location {
        uri: target.uri,
        range,
    }
}

/// Converts a `DefTarget` (with a known origin) to a wire `LocationLink`:
/// `originSelectionRange` converts `target.origin` against
/// `requesting_text` — the REQUESTING document's current text, not the
/// target's — since the origin span lives in the document that sent the
/// request. `targetRange`/`targetSelectionRange` are identical, both from
/// `target.span` (the framework advertises no distinct "selection" vs
/// "full extent" range for definitions).
fn to_location_link(state: &ServerState, requesting_text: &str, target: DefTarget) -> LocationLink {
    let range = target_range(state, &target);
    let origin_selection_range = target
        .origin
        .map(|span| position::span_to_range(requesting_text, span));
    LocationLink {
        origin_selection_range,
        target_uri: target.uri,
        target_range: range.clone(),
        target_selection_range: range,
    }
}

/// Char==UTF-16 identity conversion (subtract 1 to go from 1-based to
/// 0-based, nothing else) for an external `DefTarget` whose text isn't
/// available to convert against exactly — see `DefTarget`'s doc.
fn span_to_range_identity(span: Span) -> Range {
    Range {
        start: pos_to_position_identity(span.start),
        end: pos_to_position_identity(span.end),
    }
}

fn pos_to_position_identity(pos: Pos) -> Position {
    Position {
        line: pos.line.saturating_sub(1),
        character: pos.col.saturating_sub(1),
    }
}

fn handle_hover(
    state: &ServerState,
    writer: &mut dyn std::io::Write,
    services: &mut [&mut dyn LanguageService],
    id: &Id,
    params: serde_json::Value,
) {
    let Ok(params) = serde_json::from_value::<TextDocumentPositionParams>(params) else {
        respond_err(
            writer,
            Some(id),
            error_codes::INVALID_PARAMS,
            "invalid hover params",
        );
        return;
    };
    let uri = params.text_document.uri;
    let text = doc_text(state, &uri);
    let pos = position::pos_from_lsp(text, params.position);
    let idx = state.bindings.get(&uri).copied().unwrap_or(0);

    let result = match services[idx].hover(&uri, pos) {
        Some(content) => {
            serde_json::to_value(to_hover(text, content)).expect("Hover is always serializable")
        }
        None => serde_json::Value::Null,
    };
    respond_ok(writer, id, result);
}

/// Converts a `HoverContent` to its wire shape: plain-text `contents`
/// (v1 carries no markdown — docs/lsp.md) and `range` converted against
/// the REQUESTING document's current text.
fn to_hover(text: &str, content: HoverContent) -> Hover {
    Hover {
        contents: MarkupContent {
            kind: "plaintext".to_string(),
            value: content.text,
        },
        range: position::span_to_range(text, content.span),
    }
}

fn handle_code_action(
    state: &ServerState,
    writer: &mut dyn std::io::Write,
    services: &mut [&mut dyn LanguageService],
    id: &Id,
    params: serde_json::Value,
) {
    let Ok(params) = serde_json::from_value::<CodeActionParams>(params) else {
        respond_err(
            writer,
            Some(id),
            error_codes::INVALID_PARAMS,
            "invalid codeAction params",
        );
        return;
    };
    let uri = params.text_document.uri;
    let text = doc_text(state, &uri);
    let span = position::range_to_span(text, params.range);
    let idx = state.bindings.get(&uri).copied().unwrap_or(0);

    let actions: Vec<CodeAction> = services[idx]
        .code_actions(&uri, span)
        .into_iter()
        .map(|action| to_code_action(&uri, text, action))
        .collect();

    let result = serde_json::to_value(actions).expect("CodeAction[] is always serializable");
    respond_ok(writer, id, result);
}

/// Edits in an `Action` apply to the requesting document (the trait's
/// contract), so every edit lands under the same single `uri` key.
fn to_code_action(uri: &str, text: &str, action: Action) -> CodeAction {
    let edits = action
        .edits
        .into_iter()
        .map(|edit| TextEdit {
            range: position::span_to_range(text, edit.span),
            new_text: edit.replacement,
        })
        .collect();
    let mut changes = HashMap::new();
    changes.insert(uri.to_string(), edits);

    CodeAction {
        title: action.title,
        kind: "quickfix".to_string(),
        is_preferred: Some(action.preferred),
        edit: WorkspaceEdit { changes },
    }
}

fn handle_document_symbol(
    state: &ServerState,
    writer: &mut dyn std::io::Write,
    services: &mut [&mut dyn LanguageService],
    id: &Id,
    params: serde_json::Value,
) {
    let Ok(params) = serde_json::from_value::<DocumentSymbolParams>(params) else {
        respond_err(
            writer,
            Some(id),
            error_codes::INVALID_PARAMS,
            "invalid documentSymbol params",
        );
        return;
    };
    let uri = params.text_document.uri;
    let text = doc_text(state, &uri);
    let idx = state.bindings.get(&uri).copied().unwrap_or(0);

    let result = match services[idx].document_symbols(&uri) {
        Some(nodes) => {
            let symbols: Vec<DocumentSymbol> = nodes
                .into_iter()
                .map(|node| to_document_symbol(text, node))
                .collect();
            serde_json::to_value(symbols).expect("DocumentSymbol[] is always serializable")
        }
        None => serde_json::Value::Null,
    };
    respond_ok(writer, id, result);
}

fn to_document_symbol(text: &str, node: SymbolNode) -> DocumentSymbol {
    DocumentSymbol {
        name: node.name,
        kind: match node.kind {
            SymbolNodeKind::Namespace => symbol_kind::NAMESPACE,
            SymbolNodeKind::Function => symbol_kind::FUNCTION,
        },
        range: position::span_to_range(text, node.span),
        selection_range: position::span_to_range(text, node.selection_span),
        children: node
            .children
            .into_iter()
            .map(|child| to_document_symbol(text, child))
            .collect(),
    }
}

fn handle_semantic_tokens(
    state: &ServerState,
    writer: &mut dyn std::io::Write,
    services: &mut [&mut dyn LanguageService],
    id: &Id,
    params: serde_json::Value,
) {
    let Ok(params) = serde_json::from_value::<SemanticTokensParams>(params) else {
        respond_err(
            writer,
            Some(id),
            error_codes::INVALID_PARAMS,
            "invalid semanticTokens params",
        );
        return;
    };
    let uri = params.text_document.uri;
    let text = doc_text(state, &uri);
    let idx = state.bindings.get(&uri).copied().unwrap_or(0);
    let type_offset = state.type_offsets.get(idx).copied().unwrap_or(0);

    let result = match services[idx].semantic_tokens(&uri) {
        Some(tokens) => {
            // Relocate each token from the bound service's local legend
            // into the merged legend's index space before wire packing
            // (docs/lsp.md, capability merge). For a single service the
            // maps are identities, so `remapped == tokens` and the packed
            // bytes match a dedicated server exactly.
            let modifier_map = state
                .modifier_maps
                .get(idx)
                .map(Vec::as_slice)
                .unwrap_or(&[]);
            let remapped: Vec<SemToken> = tokens
                .iter()
                .map(|token| remap_token(*token, type_offset, modifier_map))
                .collect();
            let data = position::pack_semantic_tokens(text, &remapped);
            serde_json::to_value(SemanticTokens { data })
                .expect("SemanticTokens is always serializable")
        }
        None => serde_json::Value::Null,
    };
    respond_ok(writer, id, result);
}

/// Relocates one service-local [`SemToken`] into the merged legend's
/// index space (docs/lsp.md, capability merge): the type index shifts by
/// the service's concatenation offset; each set modifier bit moves to the
/// merged bit its per-service map names.
///
/// A modifier bit set beyond the service's declared modifier count is an
/// impossible token for a well-behaved service — it advertised N
/// modifiers yet set bit ≥ N. Such a bit is **masked off** (dropped)
/// rather than passed through: passing it through would corrupt an
/// unrelated merged bit (the merged legend is at least as wide), while
/// masking loses only the already-meaningless bit. `debug_assert` turns
/// the service bug into a test-build panic; release builds silently mask.
fn remap_token(token: SemToken, type_offset: u32, modifier_map: &[u32]) -> SemToken {
    let mut modifiers = 0u32;
    for bit in 0..u32::BITS {
        if token.modifiers & (1 << bit) == 0 {
            continue;
        }
        match modifier_map.get(bit as usize) {
            Some(&merged_bit) => modifiers |= 1 << merged_bit,
            None => debug_assert!(
                false,
                "semantic token modifier bit {bit} exceeds the service's {} declared modifiers",
                modifier_map.len()
            ),
        }
    }
    SemToken {
        span: token.span,
        token_type: token.token_type + type_offset,
        modifiers,
    }
}

fn handle_formatting(
    state: &ServerState,
    writer: &mut dyn std::io::Write,
    services: &mut [&mut dyn LanguageService],
    id: &Id,
    params: serde_json::Value,
) {
    let Ok(params) = serde_json::from_value::<DocumentFormattingParams>(params) else {
        respond_err(
            writer,
            Some(id),
            error_codes::INVALID_PARAMS,
            "invalid formatting params",
        );
        return;
    };
    let uri = params.text_document.uri;
    let text = doc_text(state, &uri);
    let idx = state.bindings.get(&uri).copied().unwrap_or(0);

    let result = match services[idx].format(&uri) {
        None => serde_json::Value::Null,
        Some(formatted) if formatted == text => {
            serde_json::to_value(Vec::<TextEdit>::new()).expect("empty TextEdit[] is serializable")
        }
        Some(formatted) => {
            let end = position::pos_to_lsp(
                text,
                Pos {
                    line: u32::MAX,
                    col: u32::MAX,
                },
            );
            let edit = TextEdit {
                range: Range {
                    start: Position {
                        line: 0,
                        character: 0,
                    },
                    end,
                },
                new_text: formatted,
            };
            serde_json::to_value(vec![edit]).expect("TextEdit[] is always serializable")
        }
    };
    respond_ok(writer, id, result);
}

fn build_initialize_result(
    services: &[&mut dyn LanguageService],
    merged: &MergedLegend,
    identity: ServerIdentity,
) -> InitializeResult {
    InitializeResult {
        capabilities: ServerCapabilities {
            position_encoding: "utf-16".to_string(),
            text_document_sync: TextDocumentSyncOptions {
                open_close: true,
                change: 1,
            },
            completion_provider: CompletionOptions {
                trigger_characters: merge_trigger_characters(services),
            },
            definition_provider: true,
            hover_provider: true,
            document_formatting_provider: true,
            document_symbol_provider: true,
            code_action_provider: CodeActionOptions {
                code_action_kinds: vec!["quickfix".to_string()],
            },
            semantic_tokens_provider: SemanticTokensOptions {
                legend: SemanticTokensLegend {
                    token_types: merged.token_types.clone(),
                    token_modifiers: merged.token_modifiers.clone(),
                },
                full: true,
            },
        },
        server_info: ServerInfoWire {
            name: identity.name.to_string(),
            version: identity.version.to_string(),
        },
    }
}

/// Sends the `client/registerCapability` request for
/// `workspace/didChangeWatchedFiles`, skipped entirely when no service
/// advertises any glob to watch. Watches the ordered dedup-union of every
/// service's globs (docs/lsp.md, capability merge).
fn send_register_capability(
    state: &mut ServerState,
    writer: &mut dyn std::io::Write,
    services: &mut [&mut dyn LanguageService],
) {
    let globs = merge_watched_globs(services);
    if globs.is_empty() {
        return;
    }

    let id = state.next_request_id;
    state.next_request_id += 1;

    let watchers = globs
        .iter()
        .map(|glob| FileSystemWatcher {
            glob_pattern: glob.clone(),
        })
        .collect();
    let params = RegistrationParams {
        registrations: vec![Registration {
            id: "workspace-watched-files".to_string(),
            method: "workspace/didChangeWatchedFiles".to_string(),
            register_options: serde_json::to_value(DidChangeWatchedFilesRegistrationOptions {
                watchers,
            })
            .expect("DidChangeWatchedFilesRegistrationOptions is always serializable"),
        }],
    };
    let payload = jsonrpc::request(
        id,
        "client/registerCapability",
        serde_json::to_value(params).expect("RegistrationParams is always serializable"),
    );
    let _ = transport::write_message(writer, &payload);
}

fn respond_ok(writer: &mut dyn std::io::Write, id: &Id, result: serde_json::Value) {
    let payload = jsonrpc::response_ok(id, result);
    let _ = transport::write_message(writer, &payload);
}

fn respond_err(writer: &mut dyn std::io::Write, id: Option<&Id>, code: i64, message: &str) {
    let payload = jsonrpc::response_err(id, code, message);
    let _ = transport::write_message(writer, &payload);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lsp::fake::FakeService;

    const TEST_IDENTITY: ServerIdentity = ServerIdentity {
        name: "test-server",
        version: "0.0.0-test",
    };

    /// A second toy service for the multi-service routing + capability-merge
    /// tests. It is deliberately built to OVERLAP `FakeService`'s legend so
    /// the merge's two asymmetric rules are observable:
    ///
    /// - its token TYPES include `"function"` (shared with `FakeService`),
    ///   so the merged `tokenTypes` list carries `"function"` **twice** —
    ///   proving types are concatenated, never deduped;
    /// - its token MODIFIERS include `"declaration"` (shared with
    ///   `FakeService`) listed AFTER its own `"deprecated"`, so the merged
    ///   `tokenModifiers` list carries `"declaration"` **once** and this
    ///   service's local bits remap non-trivially (`[deprecated→1,
    ///   declaration→0]`) — proving modifiers are dedup-unioned and the
    ///   per-service bit map is real.
    ///
    /// (The task brief sketched this service's legend as
    /// `(["kw","number"], ["deprecated"])`, but `FakeService` is pinned to
    /// types `["function"]` / modifiers `["declaration"]` by a byte-identity
    /// test, so a sketch with no shared names could not distinguish dedup
    /// from no-dedup. The concrete names here are chosen to share exactly
    /// one type and one modifier with `FakeService`, which is what actually
    /// exercises the merge.)
    ///
    /// Its diagnostics/completions/etc. are tagged `"fake2"` so a response's
    /// content reveals which service produced it — that is how the routing
    /// tests assert the binding was honored.
    struct FakeService2 {
        texts: HashMap<String, String>,
        config_revision: u32,
    }

    impl FakeService2 {
        fn new() -> Self {
            FakeService2 {
                texts: HashMap::new(),
                config_revision: 0,
            }
        }
    }

    /// Single-line, char-precise occurrences of `needle` on the first line
    /// of `text` — enough for the compact fixtures the merge tests use.
    fn first_line_spans(text: &str, needle: &str) -> Vec<Span> {
        let needle: Vec<char> = needle.chars().collect();
        let line = text.split('\n').next().unwrap_or("");
        let chars: Vec<char> = line.chars().collect();
        let mut spans = Vec::new();
        let mut i = 0;
        while i + needle.len() <= chars.len() {
            if chars[i..i + needle.len()] == needle[..] {
                let start = (i + 1) as u32;
                spans.push(Span::new(1, start, 1, start + needle.len() as u32));
                i += needle.len();
            } else {
                i += 1;
            }
        }
        spans
    }

    impl LanguageService for FakeService2 {
        fn language_id(&self) -> &'static str {
            "fake2"
        }
        fn extensions(&self) -> &'static [&'static str] {
            &[".f2"]
        }
        fn trigger_characters(&self) -> &[char] {
            // '@' is unique; '.' overlaps FakeService, so the union dedups it.
            &['@', '.']
        }
        fn token_legend(&self) -> (&'static [&'static str], &'static [&'static str]) {
            (&["kw", "function"], &["deprecated", "declaration"])
        }
        fn watched_globs(&self) -> &'static [&'static str] {
            &["**/fake2.json"]
        }
        fn did_update(&mut self, uri: &str, text: &str) -> Vec<ServiceDiagnostic> {
            self.texts.insert(uri.to_string(), text.to_string());
            first_line_spans(text, "boo")
                .into_iter()
                .map(|span| ServiceDiagnostic {
                    span,
                    severity: ServiceSeverity::Warning,
                    source: "fake2",
                    code: Some("boo-word"),
                    message: format!("boo (fake2 rev {})", self.config_revision),
                    deprecated: false,
                })
                .collect()
        }
        fn did_close(&mut self, uri: &str) {
            self.texts.remove(uri);
        }
        fn did_change_config(&mut self, _settings: serde_json::Value) {
            self.config_revision += 1;
        }
        fn completion(&mut self, _uri: &str, pos: Pos) -> Vec<Candidate> {
            vec![Candidate {
                label: "beta".to_string(),
                kind: CandidateKind::Keyword,
                replace_span: Span {
                    start: pos,
                    end: pos,
                },
                insert_text: "beta".to_string(),
                detail: None,
                deprecated: false,
            }]
        }
        fn definition(&mut self, uri: &str, _pos: Pos) -> Option<DefTarget> {
            let text = self.texts.get(uri)?;
            first_line_spans(text, "ref")
                .into_iter()
                .next()
                .map(|span| DefTarget {
                    uri: uri.to_string(),
                    span,
                    origin: None,
                })
        }
        // The `null`-shape half of the hover wire contract (docs/lsp.md):
        // `FakeService`'s `hover` is an unconditional `Some`, so a routing
        // test needs a second, bound service that always answers `None`.
        fn hover(&mut self, _uri: &str, _pos: Pos) -> Option<HoverContent> {
            None
        }
        fn code_actions(&mut self, _uri: &str, _span: Span) -> Vec<Action> {
            Vec::new()
        }
        fn document_symbols(&mut self, _uri: &str) -> Option<Vec<SymbolNode>> {
            None
        }
        fn semantic_tokens(&mut self, uri: &str) -> Option<Vec<SemToken>> {
            let text = self.texts.get(uri).map(String::as_str).unwrap_or("");
            let mut tokens: Vec<SemToken> = first_line_spans(text, "kw")
                .into_iter()
                .map(|span| SemToken {
                    span,
                    // local "kw" type index, local "deprecated" bit.
                    token_type: 0,
                    modifiers: 1 << 0,
                })
                .collect();
            // A second trigger for the OTHER local modifier bit
            // ("declaration", local bit 1) — the down-shift remap
            // direction (`semantic_tokens_local_declaration_bit_remaps_
            // down_to_merged_bit_zero` below) has nothing to key off
            // without it; "kw"'s fixed bit-0 modifier only ever exercises
            // the up-shift.
            tokens.extend(
                first_line_spans(text, "old")
                    .into_iter()
                    .map(|span| SemToken {
                        span,
                        token_type: 0,
                        // local "declaration" bit.
                        modifiers: 1 << 1,
                    }),
            );
            Some(tokens)
        }
        fn format(&mut self, uri: &str) -> Option<String> {
            let text = self.texts.get(uri)?;
            Some(text.to_uppercase())
        }
    }

    fn initialize_message(id: i64) -> serde_json::Value {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": "initialize", "params": {}})
    }

    fn did_open_message(uri: &str, version: i32, text: &str) -> serde_json::Value {
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": uri,
                    "languageId": "fake",
                    "version": version,
                    "text": text,
                },
            },
        })
    }

    fn did_open_message_lang(
        uri: &str,
        language_id: &str,
        version: i32,
        text: &str,
    ) -> serde_json::Value {
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didOpen",
            "params": {
                "textDocument": {
                    "uri": uri,
                    "languageId": language_id,
                    "version": version,
                    "text": text,
                },
            },
        })
    }

    fn did_change_message(uri: &str, version: i32, text: &str) -> serde_json::Value {
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didChange",
            "params": {
                "textDocument": {"uri": uri, "version": version},
                "contentChanges": [{"text": text}],
            },
        })
    }

    fn did_close_message(uri: &str) -> serde_json::Value {
        serde_json::json!({
            "jsonrpc": "2.0",
            "method": "textDocument/didClose",
            "params": {"textDocument": {"uri": uri}},
        })
    }

    fn request_message(id: i64, method: &str, params: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
    }

    fn notification_message(method: &str, params: serde_json::Value) -> serde_json::Value {
        serde_json::json!({"jsonrpc": "2.0", "method": method, "params": params})
    }

    fn decode_output_frames(buf: &[u8]) -> Vec<serde_json::Value> {
        let mut reader = buf;
        let mut outputs = Vec::new();
        while let Some(payload) =
            transport::read_message(&mut reader).expect("recorded output must be correctly framed")
        {
            outputs.push(
                serde_json::from_str(&payload).expect("recorded output payload must be valid json"),
            );
        }
        outputs
    }

    fn run_session(
        client_messages: &[serde_json::Value],
        service: &mut dyn LanguageService,
    ) -> (Vec<serde_json::Value>, i32) {
        run_session_multi(client_messages, &mut [service])
    }

    /// The multi-service driver: same as `run_session` but over a slice of
    /// services, for the routing/capability-merge tests below.
    fn run_session_multi(
        client_messages: &[serde_json::Value],
        services: &mut [&mut dyn LanguageService],
    ) -> (Vec<serde_json::Value>, i32) {
        let mut input = Vec::new();
        for msg in client_messages {
            transport::write_message(&mut input, &msg.to_string())
                .expect("write_message into a Vec cannot fail");
        }

        let mut output = Vec::new();
        let mut reader = &input[..];
        let exit_code = run(&mut reader, &mut output, services, TEST_IDENTITY);

        (decode_output_frames(&output), exit_code)
    }

    #[test]
    fn initialize_returns_capabilities_built_from_the_service_and_bumps_config() {
        let mut service = FakeService::new();
        let (outputs, exit_code) = run_session(
            &[serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "initialize",
                "params": {"initializationOptions": {"n": 1}},
            })],
            &mut service,
        );

        assert_eq!(outputs.len(), 1);
        // This is a deliberate byte-identity pin (docs/lsp.md); this
        // task's ONLY change to it is the added `"hoverProvider": true`
        // line — every other key is unchanged from before hover shipped.
        assert_eq!(
            outputs[0],
            serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {
                    "capabilities": {
                        "positionEncoding": "utf-16",
                        "textDocumentSync": {"openClose": true, "change": 1},
                        "completionProvider": {"triggerCharacters": ["."]},
                        "definitionProvider": true,
                        "hoverProvider": true,
                        "documentFormattingProvider": true,
                        "documentSymbolProvider": true,
                        "codeActionProvider": {"codeActionKinds": ["quickfix"]},
                        "semanticTokensProvider": {
                            "legend": {
                                "tokenTypes": ["function"],
                                "tokenModifiers": ["declaration"],
                            },
                            "full": true,
                        },
                    },
                    "serverInfo": {"name": "test-server", "version": "0.0.0-test"},
                },
            })
        );

        // EOF without an `exit` notification: dead-client exit code.
        assert_eq!(exit_code, 1);

        // initializationOptions were fed to did_change_config: one config
        // revision bump, observable through a probe did_update call.
        let diagnostics = service.did_update("file:///probe.fake", "bad");
        assert_eq!(diagnostics[0].message, "bad word (config rev 1)");
    }

    #[test]
    fn initialized_registers_the_watched_files_capability() {
        let mut service = FakeService::new();
        let (outputs, exit_code) = run_session(
            &[
                initialize_message(1),
                serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
            ],
            &mut service,
        );

        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[1]["method"], "client/registerCapability");
        assert_eq!(outputs[1]["id"], serde_json::json!(1));
        let registrations = outputs[1]["params"]["registrations"].as_array().unwrap();
        assert_eq!(registrations.len(), 1);
        assert_eq!(
            registrations[0]["method"],
            "workspace/didChangeWatchedFiles"
        );
        assert_eq!(
            registrations[0]["registerOptions"]["watchers"],
            serde_json::json!([{"globPattern": "**/fake.json"}])
        );

        assert_eq!(exit_code, 1);
    }

    #[test]
    fn initialized_skips_registration_when_watched_globs_are_empty() {
        struct NoWatchService;
        impl LanguageService for NoWatchService {
            fn language_id(&self) -> &'static str {
                "none"
            }
            fn extensions(&self) -> &'static [&'static str] {
                &[]
            }
            fn trigger_characters(&self) -> &[char] {
                &[]
            }
            fn token_legend(&self) -> (&'static [&'static str], &'static [&'static str]) {
                (&[], &[])
            }
            fn watched_globs(&self) -> &'static [&'static str] {
                &[]
            }
            fn did_update(
                &mut self,
                _uri: &str,
                _text: &str,
            ) -> Vec<crate::lsp::ServiceDiagnostic> {
                unimplemented!()
            }
            fn did_close(&mut self, _uri: &str) {}
            fn did_change_config(&mut self, _settings: serde_json::Value) {}
            fn completion(
                &mut self,
                _uri: &str,
                _pos: crate::diagnostics::Pos,
            ) -> Vec<crate::lsp::Candidate> {
                unimplemented!()
            }
            fn definition(
                &mut self,
                _uri: &str,
                _pos: crate::diagnostics::Pos,
            ) -> Option<crate::lsp::DefTarget> {
                unimplemented!()
            }
            fn hover(
                &mut self,
                _uri: &str,
                _pos: crate::diagnostics::Pos,
            ) -> Option<crate::lsp::HoverContent> {
                unimplemented!()
            }
            fn code_actions(
                &mut self,
                _uri: &str,
                _span: crate::diagnostics::Span,
            ) -> Vec<crate::lsp::Action> {
                unimplemented!()
            }
            fn document_symbols(&mut self, _uri: &str) -> Option<Vec<crate::lsp::SymbolNode>> {
                unimplemented!()
            }
            fn semantic_tokens(&mut self, _uri: &str) -> Option<Vec<crate::lsp::SemToken>> {
                unimplemented!()
            }
            fn format(&mut self, _uri: &str) -> Option<String> {
                unimplemented!()
            }
        }

        let mut service = NoWatchService;
        let (outputs, _exit_code) = run_session(
            &[
                initialize_message(1),
                serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
            ],
            &mut service,
        );

        // Only the initialize response — registerCapability is skipped entirely.
        assert_eq!(outputs.len(), 1);
    }

    #[test]
    fn request_before_initialize_is_rejected_but_session_continues() {
        let mut service = FakeService::new();
        let (outputs, exit_code) = run_session(
            &[
                serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "textDocument/completion", "params": {}}),
                initialize_message(2),
            ],
            &mut service,
        );

        assert_eq!(outputs.len(), 2);
        assert_eq!(
            outputs[0]["error"]["code"],
            serde_json::json!(error_codes::SERVER_NOT_INITIALIZED)
        );
        assert_eq!(outputs[0]["id"], serde_json::json!(1));
        assert_eq!(outputs[1]["id"], serde_json::json!(2));
        assert!(outputs[1]["result"]["capabilities"].is_object());
        assert_eq!(exit_code, 1);
    }

    #[test]
    fn notification_before_initialize_is_dropped() {
        let mut service = FakeService::new();
        let (outputs, exit_code) = run_session(
            &[serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}})],
            &mut service,
        );

        assert_eq!(outputs.len(), 0);
        assert_eq!(exit_code, 1);
    }

    #[test]
    fn exit_before_initialize_still_exits() {
        let mut service = FakeService::new();
        let (outputs, exit_code) = run_session(
            &[serde_json::json!({"jsonrpc": "2.0", "method": "exit"})],
            &mut service,
        );

        assert_eq!(outputs.len(), 0);
        assert_eq!(exit_code, 1);
    }

    #[test]
    fn second_initialize_is_invalid_request() {
        let mut service = FakeService::new();
        let (outputs, _exit_code) = run_session(
            &[initialize_message(1), initialize_message(2)],
            &mut service,
        );

        assert_eq!(outputs.len(), 2);
        assert_eq!(
            outputs[1]["error"]["code"],
            serde_json::json!(error_codes::INVALID_REQUEST)
        );
        assert_eq!(outputs[1]["id"], serde_json::json!(2));
    }

    #[test]
    fn shutdown_then_exit_returns_zero() {
        let mut service = FakeService::new();
        let (outputs, exit_code) = run_session(
            &[
                initialize_message(1),
                serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown", "params": null}),
                serde_json::json!({"jsonrpc": "2.0", "method": "exit"}),
            ],
            &mut service,
        );

        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[1]["id"], serde_json::json!(2));
        assert_eq!(outputs[1]["result"], serde_json::Value::Null);
        assert_eq!(exit_code, 0);
    }

    #[test]
    fn exit_without_shutdown_returns_one() {
        let mut service = FakeService::new();
        let (outputs, exit_code) = run_session(
            &[
                initialize_message(1),
                serde_json::json!({"jsonrpc": "2.0", "method": "exit"}),
            ],
            &mut service,
        );

        assert_eq!(outputs.len(), 1);
        assert_eq!(exit_code, 1);
    }

    #[test]
    fn eof_without_exit_returns_one() {
        let mut service = FakeService::new();
        let (_outputs, exit_code) = run_session(&[initialize_message(1)], &mut service);
        assert_eq!(exit_code, 1);
    }

    #[test]
    fn transport_error_is_treated_like_a_dead_client() {
        // No Content-Length header, no colon at all on the first line:
        // read_message returns a TransportError. The loop must treat
        // that like a dead client rather than panicking or hanging.
        let raw = b"not a valid header block\r\n\r\n";
        let mut reader = &raw[..];
        let mut output = Vec::new();
        let mut service = FakeService::new();
        let mut services: [&mut dyn LanguageService; 1] = [&mut service];

        let exit_code = run(&mut reader, &mut output, &mut services, TEST_IDENTITY);

        assert_eq!(exit_code, 1);
        assert!(output.is_empty());
    }

    #[test]
    fn unknown_request_after_init_is_method_not_found() {
        let mut service = FakeService::new();
        let (outputs, _exit_code) = run_session(
            &[
                initialize_message(1),
                serde_json::json!({"jsonrpc": "2.0", "id": 2, "method": "foo/bar", "params": {}}),
            ],
            &mut service,
        );

        assert_eq!(outputs.len(), 2);
        assert_eq!(
            outputs[1]["error"]["code"],
            serde_json::json!(error_codes::METHOD_NOT_FOUND)
        );
        assert_eq!(outputs[1]["id"], serde_json::json!(2));
    }

    #[test]
    fn dollar_cancel_request_notification_produces_no_output() {
        let mut service = FakeService::new();
        let (outputs, exit_code) = run_session(
            &[
                initialize_message(1),
                serde_json::json!({"jsonrpc": "2.0", "method": "$/cancelRequest", "params": {"id": 1}}),
            ],
            &mut service,
        );

        assert_eq!(outputs.len(), 1);
        assert_eq!(exit_code, 1);
    }

    #[test]
    fn incoming_response_message_is_dropped_silently() {
        let mut service = FakeService::new();
        let (outputs, exit_code) = run_session(
            &[
                initialize_message(1),
                serde_json::json!({"jsonrpc": "2.0", "id": 99, "result": {}}),
            ],
            &mut service,
        );

        assert_eq!(outputs.len(), 1);
        assert_eq!(exit_code, 1);
    }

    #[test]
    fn malformed_json_frame_gets_parse_error_and_session_continues() {
        let mut input = Vec::new();
        transport::write_message(&mut input, "not json at all").unwrap();
        transport::write_message(&mut input, &initialize_message(1).to_string()).unwrap();

        let mut output = Vec::new();
        let mut reader = &input[..];
        let mut service = FakeService::new();
        let mut services: [&mut dyn LanguageService; 1] = [&mut service];
        let exit_code = run(&mut reader, &mut output, &mut services, TEST_IDENTITY);
        let outputs = decode_output_frames(&output);

        assert_eq!(outputs.len(), 2);
        assert_eq!(
            outputs[0]["error"]["code"],
            serde_json::json!(error_codes::PARSE_ERROR)
        );
        assert!(outputs[0]["id"].is_null());
        assert_eq!(outputs[1]["id"], serde_json::json!(1));
        assert!(outputs[1]["result"]["capabilities"].is_object());
        assert_eq!(exit_code, 1);
    }

    #[test]
    fn shape_error_frame_gets_invalid_request_and_session_continues() {
        let mut input = Vec::new();
        transport::write_message(&mut input, "[1,2,3]").unwrap();
        transport::write_message(&mut input, &initialize_message(1).to_string()).unwrap();

        let mut output = Vec::new();
        let mut reader = &input[..];
        let mut service = FakeService::new();
        let mut services: [&mut dyn LanguageService; 1] = [&mut service];
        let exit_code = run(&mut reader, &mut output, &mut services, TEST_IDENTITY);
        let outputs = decode_output_frames(&output);

        assert_eq!(outputs.len(), 2);
        assert_eq!(
            outputs[0]["error"]["code"],
            serde_json::json!(error_codes::INVALID_REQUEST)
        );
        assert!(outputs[0]["id"].is_null());
        assert_eq!(outputs[1]["id"], serde_json::json!(1));
        assert_eq!(exit_code, 1);
    }

    #[test]
    fn did_open_publishes_full_diagnostics_set_with_document_version() {
        let mut service = FakeService::new();
        let (outputs, _exit_code) = run_session(
            &[
                initialize_message(1),
                did_open_message("file:///a.fake", 1, "ok bad ok"),
            ],
            &mut service,
        );

        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[1]["method"], "textDocument/publishDiagnostics");
        assert_eq!(
            outputs[1]["params"],
            serde_json::json!({
                "uri": "file:///a.fake",
                "version": 1,
                "diagnostics": [
                    {
                        "range": {
                            "start": {"line": 0, "character": 3},
                            "end": {"line": 0, "character": 6},
                        },
                        "severity": 1,
                        "code": "bad-word",
                        "source": "fake",
                        "message": "bad word (config rev 0)",
                    },
                ],
            })
        );
    }

    #[test]
    fn did_change_republishes_new_version_with_empty_diagnostics_when_clean() {
        let mut service = FakeService::new();
        let (outputs, _exit_code) = run_session(
            &[
                initialize_message(1),
                did_open_message("file:///a.fake", 1, "ok bad ok"),
                did_change_message("file:///a.fake", 2, "all clear"),
            ],
            &mut service,
        );

        assert_eq!(outputs.len(), 3);
        assert_eq!(outputs[2]["method"], "textDocument/publishDiagnostics");
        assert_eq!(
            outputs[2]["params"],
            serde_json::json!({
                "uri": "file:///a.fake",
                "version": 2,
                "diagnostics": [],
            })
        );
    }

    #[test]
    fn did_close_publishes_an_empty_set_with_version_omitted() {
        let mut service = FakeService::new();
        let (outputs, _exit_code) = run_session(
            &[
                initialize_message(1),
                did_open_message("file:///a.fake", 1, "ok bad ok"),
                did_change_message("file:///a.fake", 2, "bad"),
                did_close_message("file:///a.fake"),
            ],
            &mut service,
        );

        assert_eq!(outputs.len(), 4);
        let last = &outputs[3];
        assert_eq!(last["method"], "textDocument/publishDiagnostics");
        assert_eq!(
            last["params"],
            serde_json::json!({
                "uri": "file:///a.fake",
                "diagnostics": [],
            })
        );
        assert!(last["params"].get("version").is_none());
    }

    #[test]
    fn did_open_reports_multiple_diagnostics_in_source_order() {
        let mut service = FakeService::new();
        let (outputs, _exit_code) = run_session(
            &[
                initialize_message(1),
                did_open_message("file:///a.fake", 1, "bad bad"),
            ],
            &mut service,
        );

        assert_eq!(outputs.len(), 2);
        let diagnostics = outputs[1]["params"]["diagnostics"].as_array().unwrap();
        assert_eq!(diagnostics.len(), 2);
        assert_eq!(
            diagnostics[0]["range"],
            serde_json::json!({
                "start": {"line": 0, "character": 0},
                "end": {"line": 0, "character": 3},
            })
        );
        assert_eq!(
            diagnostics[1]["range"],
            serde_json::json!({
                "start": {"line": 0, "character": 4},
                "end": {"line": 0, "character": 7},
            })
        );
    }

    #[test]
    fn completion_returns_text_edit_shaped_items_with_the_service_kind() {
        let mut service = FakeService::new();
        let (outputs, _exit_code) = run_session(
            &[
                initialize_message(1),
                did_open_message("file:///a.fake", 1, "hi"),
                request_message(
                    2,
                    "textDocument/completion",
                    serde_json::json!({
                        "textDocument": {"uri": "file:///a.fake"},
                        "position": {"line": 0, "character": 0},
                    }),
                ),
            ],
            &mut service,
        );

        assert_eq!(outputs.len(), 3);
        assert_eq!(outputs[2]["id"], serde_json::json!(2));
        assert_eq!(
            outputs[2]["result"],
            serde_json::json!([
                {
                    "label": "alpha",
                    "kind": 3,
                    "textEdit": {
                        "range": {
                            "start": {"line": 0, "character": 0},
                            "end": {"line": 0, "character": 0},
                        },
                        "newText": "alpha",
                    },
                },
            ])
        );
    }

    #[test]
    fn definition_hits_the_first_def_occurrence_and_misses_return_null() {
        let mut service = FakeService::new();
        let (outputs, _exit_code) = run_session(
            &[
                initialize_message(1),
                did_open_message("file:///hit.fake", 1, "x def def"),
                did_open_message("file:///miss.fake", 1, "no such word"),
                request_message(
                    2,
                    "textDocument/definition",
                    serde_json::json!({
                        "textDocument": {"uri": "file:///hit.fake"},
                        "position": {"line": 0, "character": 0},
                    }),
                ),
                request_message(
                    3,
                    "textDocument/definition",
                    serde_json::json!({
                        "textDocument": {"uri": "file:///miss.fake"},
                        "position": {"line": 0, "character": 0},
                    }),
                ),
            ],
            &mut service,
        );

        assert_eq!(outputs.len(), 5);
        assert_eq!(outputs[3]["id"], serde_json::json!(2));
        assert_eq!(
            outputs[3]["result"],
            serde_json::json!({
                "uri": "file:///hit.fake",
                "range": {
                    "start": {"line": 0, "character": 2},
                    "end": {"line": 0, "character": 5},
                },
            })
        );
        assert_eq!(outputs[4]["id"], serde_json::json!(3));
        assert_eq!(outputs[4]["result"], serde_json::Value::Null);
    }

    #[test]
    fn definition_with_link_support_returns_a_location_link_with_the_origin_range() {
        let mut service = FakeService::new();
        let (outputs, _exit_code) = run_session(
            &[
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "capabilities": {
                            "textDocument": {"definition": {"linkSupport": true}},
                        },
                    },
                }),
                did_open_message("file:///hit.fake", 1, "x def def"),
                request_message(
                    2,
                    "textDocument/definition",
                    serde_json::json!({
                        "textDocument": {"uri": "file:///hit.fake"},
                        "position": {"line": 0, "character": 0},
                    }),
                ),
            ],
            &mut service,
        );

        assert_eq!(outputs.len(), 3);
        assert_eq!(outputs[2]["id"], serde_json::json!(2));
        assert_eq!(
            outputs[2]["result"],
            serde_json::json!([
                {
                    "originSelectionRange": {
                        "start": {"line": 0, "character": 2},
                        "end": {"line": 0, "character": 5},
                    },
                    "targetUri": "file:///hit.fake",
                    "targetRange": {
                        "start": {"line": 0, "character": 2},
                        "end": {"line": 0, "character": 5},
                    },
                    "targetSelectionRange": {
                        "start": {"line": 0, "character": 2},
                        "end": {"line": 0, "character": 5},
                    },
                },
            ])
        );
    }

    #[test]
    fn definition_with_link_support_but_no_origin_falls_back_to_a_plain_location() {
        // A minimal service whose `definition` never sets an origin —
        // exercises the fallback half of the `definition_link_support &&
        // target.origin.is_some()` gate even though the client declared
        // linkSupport. Mirrors `NoWatchService` above: this session sends
        // no didOpen, so `handle_definition` converts positions against
        // an unopened (empty) document — `doc_text` tolerates that — and
        // only `definition` is ever called; everything else on the trait
        // is `unimplemented!()`.
        struct OriginNoneService;

        impl LanguageService for OriginNoneService {
            fn language_id(&self) -> &'static str {
                "origin-none"
            }
            fn extensions(&self) -> &'static [&'static str] {
                &[]
            }
            fn trigger_characters(&self) -> &[char] {
                &[]
            }
            fn token_legend(&self) -> (&'static [&'static str], &'static [&'static str]) {
                (&[], &[])
            }
            fn watched_globs(&self) -> &'static [&'static str] {
                &[]
            }
            fn did_update(&mut self, _uri: &str, _text: &str) -> Vec<ServiceDiagnostic> {
                unimplemented!()
            }
            fn did_close(&mut self, _uri: &str) {}
            fn did_change_config(&mut self, _settings: serde_json::Value) {}
            fn completion(&mut self, _uri: &str, _pos: Pos) -> Vec<Candidate> {
                unimplemented!()
            }
            fn definition(&mut self, _uri: &str, _pos: Pos) -> Option<DefTarget> {
                Some(DefTarget {
                    uri: "file:///origin-none.fake".to_string(),
                    span: Span::new(1, 1, 1, 4),
                    origin: None,
                })
            }
            fn hover(&mut self, _uri: &str, _pos: Pos) -> Option<HoverContent> {
                unimplemented!()
            }
            fn code_actions(&mut self, _uri: &str, _span: Span) -> Vec<Action> {
                unimplemented!()
            }
            fn document_symbols(&mut self, _uri: &str) -> Option<Vec<SymbolNode>> {
                unimplemented!()
            }
            fn semantic_tokens(&mut self, _uri: &str) -> Option<Vec<crate::lsp::SemToken>> {
                unimplemented!()
            }
            fn format(&mut self, _uri: &str) -> Option<String> {
                unimplemented!()
            }
        }

        let mut service = OriginNoneService;
        let (outputs, _exit_code) = run_session(
            &[
                serde_json::json!({
                    "jsonrpc": "2.0",
                    "id": 1,
                    "method": "initialize",
                    "params": {
                        "capabilities": {
                            "textDocument": {"definition": {"linkSupport": true}},
                        },
                    },
                }),
                request_message(
                    2,
                    "textDocument/definition",
                    serde_json::json!({
                        "textDocument": {"uri": "file:///origin-none.fake"},
                        "position": {"line": 0, "character": 0},
                    }),
                ),
            ],
            &mut service,
        );

        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[1]["id"], serde_json::json!(2));
        assert_eq!(
            outputs[1]["result"],
            serde_json::json!({
                "uri": "file:///origin-none.fake",
                "range": {
                    "start": {"line": 0, "character": 0},
                    "end": {"line": 0, "character": 3},
                },
            })
        );
    }

    #[test]
    fn hover_routes_to_the_bound_service_and_renders_both_shapes() {
        // `FakeService`'s `hover` is an unconditional canned `Some`;
        // `FakeService2`'s is an unconditional `None` — a single session
        // binding one document to each proves both the wire shape AND
        // that hover routes through the per-URI binding like every other
        // feature request, not just service 0.
        let mut s1 = FakeService::new();
        let mut s2 = FakeService2::new();
        let (outputs, _exit) = run_session_multi(
            &[
                initialize_message(1),
                did_open_message("file:///a.fake", 1, "hi"),
                did_open_message_lang("file:///b.f2", "fake2", 1, "hi"),
                request_message(
                    2,
                    "textDocument/hover",
                    serde_json::json!({
                        "textDocument": {"uri": "file:///a.fake"},
                        "position": {"line": 0, "character": 0},
                    }),
                ),
                request_message(
                    3,
                    "textDocument/hover",
                    serde_json::json!({
                        "textDocument": {"uri": "file:///b.f2"},
                        "position": {"line": 0, "character": 0},
                    }),
                ),
            ],
            &mut [&mut s1, &mut s2],
        );

        assert_eq!(outputs.len(), 5);
        assert_eq!(outputs[3]["id"], serde_json::json!(2));
        assert_eq!(
            outputs[3]["result"],
            serde_json::json!({
                "contents": {"kind": "plaintext", "value": "fake hover text"},
                "range": {
                    "start": {"line": 0, "character": 0},
                    "end": {"line": 0, "character": 0},
                },
            })
        );
        assert_eq!(outputs[4]["id"], serde_json::json!(3));
        assert_eq!(outputs[4]["result"], serde_json::Value::Null);
    }

    #[test]
    fn deprecated_diagnostics_and_tagged_candidates_carry_their_wire_fields() {
        // A minimal service purpose-built to prove the two new wire
        // mappings in one session: a deprecated `ServiceDiagnostic`
        // publishes `"tags":[2]` (a non-deprecated one has no `tags` key
        // at all) and a `Candidate` with `detail`/`deprecated` reaches
        // completion's wire shape with `detail` + `"tags":[1]` (one with
        // neither omits both keys).
        struct TagsService;

        impl LanguageService for TagsService {
            fn language_id(&self) -> &'static str {
                "tags"
            }
            fn extensions(&self) -> &'static [&'static str] {
                &[".tags"]
            }
            fn trigger_characters(&self) -> &[char] {
                &[]
            }
            fn token_legend(&self) -> (&'static [&'static str], &'static [&'static str]) {
                (&[], &[])
            }
            fn watched_globs(&self) -> &'static [&'static str] {
                &[]
            }
            fn did_update(&mut self, _uri: &str, _text: &str) -> Vec<ServiceDiagnostic> {
                vec![
                    ServiceDiagnostic {
                        span: Span::new(1, 1, 1, 4),
                        severity: ServiceSeverity::Warning,
                        source: "tags",
                        code: Some("deprecated-word"),
                        message: "call to deprecated function 'old'".to_string(),
                        deprecated: true,
                    },
                    ServiceDiagnostic {
                        span: Span::new(1, 5, 1, 8),
                        severity: ServiceSeverity::Warning,
                        source: "tags",
                        code: Some("plain-word"),
                        message: "plain".to_string(),
                        deprecated: false,
                    },
                ]
            }
            fn did_close(&mut self, _uri: &str) {}
            fn did_change_config(&mut self, _settings: serde_json::Value) {}
            fn completion(&mut self, _uri: &str, pos: Pos) -> Vec<Candidate> {
                vec![
                    Candidate {
                        label: "old".to_string(),
                        kind: CandidateKind::Function,
                        replace_span: Span {
                            start: pos,
                            end: pos,
                        },
                        insert_text: "old".to_string(),
                        detail: Some("ns::old".to_string()),
                        deprecated: true,
                    },
                    Candidate {
                        label: "plain".to_string(),
                        kind: CandidateKind::Value,
                        replace_span: Span {
                            start: pos,
                            end: pos,
                        },
                        insert_text: "plain".to_string(),
                        detail: None,
                        deprecated: false,
                    },
                ]
            }
            fn definition(&mut self, _uri: &str, _pos: Pos) -> Option<DefTarget> {
                unimplemented!()
            }
            fn hover(&mut self, _uri: &str, _pos: Pos) -> Option<HoverContent> {
                unimplemented!()
            }
            fn code_actions(&mut self, _uri: &str, _span: Span) -> Vec<Action> {
                unimplemented!()
            }
            fn document_symbols(&mut self, _uri: &str) -> Option<Vec<SymbolNode>> {
                unimplemented!()
            }
            fn semantic_tokens(&mut self, _uri: &str) -> Option<Vec<SemToken>> {
                unimplemented!()
            }
            fn format(&mut self, _uri: &str) -> Option<String> {
                unimplemented!()
            }
        }

        let mut service = TagsService;
        let (outputs, _exit) = run_session(
            &[
                initialize_message(1),
                did_open_message_lang("file:///a.tags", "tags", 1, "anything"),
                request_message(
                    2,
                    "textDocument/completion",
                    serde_json::json!({
                        "textDocument": {"uri": "file:///a.tags"},
                        "position": {"line": 0, "character": 0},
                    }),
                ),
            ],
            &mut service,
        );

        assert_eq!(outputs.len(), 3);
        let diagnostics = outputs[1]["params"]["diagnostics"].as_array().unwrap();
        assert_eq!(diagnostics[0]["tags"], serde_json::json!([2]));
        assert!(diagnostics[1].get("tags").is_none(), "{:?}", diagnostics[1]);

        let items = outputs[2]["result"].as_array().unwrap();
        assert_eq!(items[0]["detail"], serde_json::json!("ns::old"));
        assert_eq!(items[0]["tags"], serde_json::json!([1]));
        assert!(items[1].get("detail").is_none(), "{:?}", items[1]);
        assert!(items[1].get("tags").is_none(), "{:?}", items[1]);
    }

    #[test]
    fn code_action_returns_the_quickfix_for_an_overlapping_bad_span() {
        let mut service = FakeService::new();
        let (outputs, _exit_code) = run_session(
            &[
                initialize_message(1),
                did_open_message("file:///a.fake", 1, "bad bad"),
                request_message(
                    2,
                    "textDocument/codeAction",
                    serde_json::json!({
                        "textDocument": {"uri": "file:///a.fake"},
                        "range": {
                            "start": {"line": 0, "character": 0},
                            "end": {"line": 0, "character": 3},
                        },
                    }),
                ),
            ],
            &mut service,
        );

        assert_eq!(outputs.len(), 3);
        assert_eq!(outputs[2]["id"], serde_json::json!(2));
        assert_eq!(
            outputs[2]["result"],
            serde_json::json!([
                {
                    "title": "remove bad",
                    "kind": "quickfix",
                    "isPreferred": true,
                    "edit": {
                        "changes": {
                            "file:///a.fake": [
                                {
                                    "range": {
                                        "start": {"line": 0, "character": 0},
                                        "end": {"line": 0, "character": 3},
                                    },
                                    "newText": "",
                                },
                            ],
                        },
                    },
                },
            ])
        );
    }

    #[test]
    fn document_symbol_returns_the_root_node_with_function_kind() {
        let mut service = FakeService::new();
        let (outputs, _exit_code) = run_session(
            &[
                initialize_message(1),
                did_open_message("file:///a.fake", 1, "fn one\nfn two"),
                request_message(
                    2,
                    "textDocument/documentSymbol",
                    serde_json::json!({"textDocument": {"uri": "file:///a.fake"}}),
                ),
            ],
            &mut service,
        );

        assert_eq!(outputs.len(), 3);
        assert_eq!(outputs[2]["id"], serde_json::json!(2));
        assert_eq!(
            outputs[2]["result"],
            serde_json::json!([
                {
                    "name": "root",
                    "kind": 12,
                    "range": {
                        "start": {"line": 0, "character": 0},
                        "end": {"line": 0, "character": 6},
                    },
                    "selectionRange": {
                        "start": {"line": 0, "character": 0},
                        "end": {"line": 0, "character": 6},
                    },
                    "children": [],
                },
            ])
        );
    }

    #[test]
    fn semantic_tokens_returns_packed_data_for_two_fn_occurrences() {
        let mut service = FakeService::new();
        let (outputs, _exit_code) = run_session(
            &[
                initialize_message(1),
                did_open_message("file:///a.fake", 1, "fn one\nfn two"),
                request_message(
                    2,
                    "textDocument/semanticTokens/full",
                    serde_json::json!({"textDocument": {"uri": "file:///a.fake"}}),
                ),
            ],
            &mut service,
        );

        assert_eq!(outputs.len(), 3);
        assert_eq!(outputs[2]["id"], serde_json::json!(2));
        // Two "fn" tokens (function/declaration): line1 cols1..3, line2
        // cols1..3, both ASCII — hand-computed relative packing.
        assert_eq!(
            outputs[2]["result"],
            serde_json::json!({"data": [0, 0, 2, 0, 1, 1, 0, 2, 0, 1]})
        );
    }

    #[test]
    fn formatting_returns_one_whole_document_edit_when_tabs_are_present() {
        let mut service = FakeService::new();
        let (outputs, _exit_code) = run_session(
            &[
                initialize_message(1),
                did_open_message("file:///a.fake", 1, "a\tb"),
                request_message(
                    2,
                    "textDocument/formatting",
                    serde_json::json!({"textDocument": {"uri": "file:///a.fake"}}),
                ),
            ],
            &mut service,
        );

        assert_eq!(outputs.len(), 3);
        assert_eq!(outputs[2]["id"], serde_json::json!(2));
        assert_eq!(
            outputs[2]["result"],
            serde_json::json!([
                {
                    "range": {
                        "start": {"line": 0, "character": 0},
                        "end": {"line": 0, "character": 3},
                    },
                    "newText": "a    b",
                },
            ])
        );
    }

    #[test]
    fn formatting_returns_empty_array_when_the_text_is_already_clean() {
        let mut service = FakeService::new();
        let (outputs, _exit_code) = run_session(
            &[
                initialize_message(1),
                did_open_message("file:///a.fake", 1, "a b"),
                request_message(
                    2,
                    "textDocument/formatting",
                    serde_json::json!({"textDocument": {"uri": "file:///a.fake"}}),
                ),
            ],
            &mut service,
        );

        assert_eq!(outputs.len(), 3);
        assert_eq!(outputs[2]["result"], serde_json::json!([]));
    }

    #[test]
    fn did_change_configuration_republishes_every_open_document_with_bumped_revision() {
        let mut service = FakeService::new();
        let (outputs, _exit_code) = run_session(
            &[
                initialize_message(1),
                did_open_message("file:///a.fake", 1, "bad"),
                did_open_message("file:///b.fake", 1, "bad"),
                notification_message(
                    "workspace/didChangeConfiguration",
                    serde_json::json!({"settings": {"n": 1}}),
                ),
            ],
            &mut service,
        );

        // init response, publish(a), publish(b), then exactly two
        // republishes (URI-sorted) with the bumped revision embedded.
        assert_eq!(outputs.len(), 5);
        assert_eq!(outputs[3]["method"], "textDocument/publishDiagnostics");
        assert_eq!(outputs[3]["params"]["uri"], "file:///a.fake");
        assert_eq!(
            outputs[3]["params"]["diagnostics"][0]["message"],
            "bad word (config rev 1)"
        );
        assert_eq!(outputs[4]["method"], "textDocument/publishDiagnostics");
        assert_eq!(outputs[4]["params"]["uri"], "file:///b.fake");
        assert_eq!(
            outputs[4]["params"]["diagnostics"][0]["message"],
            "bad word (config rev 1)"
        );
    }

    #[test]
    fn did_change_watched_files_republishes_every_open_document() {
        let mut service = FakeService::new();
        let (outputs, _exit_code) = run_session(
            &[
                initialize_message(1),
                did_open_message("file:///a.fake", 1, "bad"),
                did_open_message("file:///b.fake", 1, "bad"),
                notification_message(
                    "workspace/didChangeWatchedFiles",
                    serde_json::json!({"changes": [{"uri": "file:///fake.json", "type": 2}]}),
                ),
            ],
            &mut service,
        );

        // Same republish-all sweep as didChangeConfiguration, triggered
        // by the watched-files notification instead — no config bump
        // this time, so the embedded revision is unchanged.
        assert_eq!(outputs.len(), 5);
        assert_eq!(outputs[3]["params"]["uri"], "file:///a.fake");
        assert_eq!(
            outputs[3]["params"]["diagnostics"][0]["message"],
            "bad word (config rev 0)"
        );
        assert_eq!(outputs[4]["params"]["uri"], "file:///b.fake");
        assert_eq!(
            outputs[4]["params"]["diagnostics"][0]["message"],
            "bad word (config rev 0)"
        );
    }

    #[test]
    fn panic_probe_did_open_is_contained_and_the_session_stays_alive() {
        let mut service = FakeService::new();
        let (outputs, _exit_code) = run_session(
            &[
                initialize_message(1),
                did_open_message("file:///panic.fake", 1, "panic-now"),
                did_open_message("file:///healthy.fake", 2, "hi"),
                request_message(
                    3,
                    "textDocument/completion",
                    serde_json::json!({
                        "textDocument": {"uri": "file:///healthy.fake"},
                        "position": {"line": 0, "character": 0},
                    }),
                ),
            ],
            &mut service,
        );

        // init response, NO publish for the panicking didOpen, the
        // healthy doc's publish, then a normal completion response —
        // the contained panic produced no output of its own and did
        // not disturb anything after it.
        assert_eq!(outputs.len(), 3);
        assert_eq!(outputs[1]["method"], "textDocument/publishDiagnostics");
        assert_eq!(outputs[1]["params"]["uri"], "file:///healthy.fake");
        assert_eq!(outputs[2]["id"], serde_json::json!(3));
        assert!(outputs[2]["result"].is_array());
    }

    #[test]
    fn completion_request_panic_is_contained_as_an_internal_error_response() {
        let mut service = FakeService::new();
        let (outputs, exit_code) = run_session(
            &[
                initialize_message(1),
                did_open_message("file:///panic.fake", 1, "panic-now"),
                request_message(
                    2,
                    "textDocument/completion",
                    serde_json::json!({
                        "textDocument": {"uri": "file:///panic.fake"},
                        "position": {"line": 0, "character": 0},
                    }),
                ),
                serde_json::json!({"jsonrpc": "2.0", "id": 3, "method": "shutdown", "params": null}),
                serde_json::json!({"jsonrpc": "2.0", "method": "exit"}),
            ],
            &mut service,
        );

        // init response, no publish for the panicking didOpen, the
        // panicking completion request answers -32603, and the session
        // is still alive afterward (shutdown still works, exit is
        // clean).
        assert_eq!(outputs.len(), 3);
        assert_eq!(outputs[1]["id"], serde_json::json!(2));
        assert_eq!(
            outputs[1]["error"]["code"],
            serde_json::json!(error_codes::INTERNAL_ERROR)
        );
        assert_eq!(outputs[1]["error"]["message"], "fake service panic");
        assert_eq!(outputs[2]["id"], serde_json::json!(3));
        assert_eq!(outputs[2]["result"], serde_json::Value::Null);
        assert_eq!(exit_code, 0);
    }

    #[test]
    fn full_spec_shaped_session_runs_every_stage_and_exits_cleanly() {
        let mut service = FakeService::new();
        let (outputs, exit_code) = run_session(
            &[
                initialize_message(1),
                serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
                did_open_message("file:///a.fake", 1, "def bad"),
                request_message(
                    2,
                    "textDocument/completion",
                    serde_json::json!({
                        "textDocument": {"uri": "file:///a.fake"},
                        "position": {"line": 0, "character": 0},
                    }),
                ),
                request_message(
                    3,
                    "textDocument/definition",
                    serde_json::json!({
                        "textDocument": {"uri": "file:///a.fake"},
                        "position": {"line": 0, "character": 0},
                    }),
                ),
                request_message(
                    4,
                    "textDocument/formatting",
                    serde_json::json!({"textDocument": {"uri": "file:///a.fake"}}),
                ),
                did_change_message("file:///a.fake", 2, "bad"),
                notification_message(
                    "workspace/didChangeConfiguration",
                    serde_json::json!({"settings": {"n": 1}}),
                ),
                notification_message(
                    "workspace/didChangeWatchedFiles",
                    serde_json::json!({"changes": []}),
                ),
                serde_json::json!({"jsonrpc": "2.0", "id": 5, "method": "shutdown", "params": null}),
                serde_json::json!({"jsonrpc": "2.0", "method": "exit"}),
            ],
            &mut service,
        );

        // init, registerCapability, publish(open), completion,
        // definition, formatting, publish(change), publish(config
        // sweep), publish(watched-files sweep), shutdown = 10 frames;
        // exit produces none.
        assert_eq!(outputs.len(), 10);
        assert_eq!(outputs[0]["id"], serde_json::json!(1));
        assert_eq!(outputs[1]["method"], "client/registerCapability");
        assert_eq!(outputs[2]["method"], "textDocument/publishDiagnostics");

        assert_eq!(outputs[3]["id"], serde_json::json!(2));
        assert_eq!(outputs[3]["result"].as_array().unwrap().len(), 1);

        assert_eq!(outputs[4]["id"], serde_json::json!(3));
        assert!(outputs[4]["result"].is_object());

        assert_eq!(outputs[5]["id"], serde_json::json!(4));
        assert_eq!(outputs[5]["result"], serde_json::json!([]));

        assert_eq!(outputs[6]["method"], "textDocument/publishDiagnostics");
        assert_eq!(outputs[6]["params"]["version"], serde_json::json!(2));

        assert_eq!(outputs[7]["method"], "textDocument/publishDiagnostics");
        assert_eq!(
            outputs[7]["params"]["diagnostics"][0]["message"],
            "bad word (config rev 1)"
        );
        assert_eq!(outputs[8]["method"], "textDocument/publishDiagnostics");
        assert_eq!(
            outputs[8]["params"]["diagnostics"][0]["message"],
            "bad word (config rev 1)"
        );

        assert_eq!(outputs[9]["id"], serde_json::json!(5));
        assert_eq!(outputs[9]["result"], serde_json::Value::Null);
        assert_eq!(exit_code, 0);
    }

    // --- Multi-service routing + capability merge (docs/lsp.md) ---------
    //
    // These drive two services — service 0 = `FakeService` ("fake"),
    // service 1 = `FakeService2` ("fake2") — and assert each behavior on
    // the wire. Test (8) (single-service byte-identity) is proven both by
    // every single-service test above still passing unchanged through the
    // slice-based `run`, and by `single_service_capabilities_are_byte_\
    // identical_to_a_dedicated_server` below.

    /// (1) Two didOpens bind by languageId; each didOpen's publish runs the
    /// *bound* service's `did_update`, revealed by the diagnostic source.
    #[test]
    fn two_did_opens_route_did_update_to_the_bound_service() {
        let mut s1 = FakeService::new();
        let mut s2 = FakeService2::new();
        let (outputs, _exit) = run_session_multi(
            &[
                initialize_message(1),
                did_open_message_lang("file:///a.fake", "fake", 1, "bad"),
                did_open_message_lang("file:///b.f2", "fake2", 1, "boo"),
            ],
            &mut [&mut s1, &mut s2],
        );

        assert_eq!(outputs.len(), 3);
        assert_eq!(outputs[1]["params"]["uri"], "file:///a.fake");
        assert_eq!(outputs[1]["params"]["diagnostics"][0]["source"], "fake");
        assert_eq!(outputs[2]["params"]["uri"], "file:///b.f2");
        assert_eq!(outputs[2]["params"]["diagnostics"][0]["source"], "fake2");
        assert_eq!(outputs[2]["params"]["diagnostics"][0]["code"], "boo-word");
    }

    /// (2) An unknown languageId falls back to the URI-extension match.
    #[test]
    fn unknown_language_id_falls_back_to_the_extension_binding() {
        let mut s1 = FakeService::new();
        let mut s2 = FakeService2::new();
        let (outputs, _exit) = run_session_multi(
            &[
                initialize_message(1),
                // "plaintext" matches no service's languageId; ".f2" does.
                did_open_message_lang("file:///foo.f2", "plaintext", 1, "boo"),
            ],
            &mut [&mut s1, &mut s2],
        );

        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[1]["params"]["diagnostics"][0]["source"], "fake2");
    }

    /// (2b) Neither languageId NOR the URI extension matches ANY service
    /// — `bind_service`'s final fallback (the `eprintln!`-noted branch,
    /// distinct from (2) above, which still resolves through the
    /// extension match). Routing is observed the same way as elsewhere
    /// in this block: through which service's `did_update` produced the
    /// published diagnostic, not by trying to capture the stderr note.
    #[test]
    fn unmatched_language_id_and_extension_falls_back_to_service_zero() {
        let mut s1 = FakeService::new();
        let mut s2 = FakeService2::new();
        let (outputs, _exit) = run_session_multi(
            &[
                initialize_message(1),
                // "plaintext" matches no languageId; ".unknown" matches
                // no service's extensions() either.
                did_open_message_lang("file:///x.unknown", "plaintext", 1, "bad"),
            ],
            &mut [&mut s1, &mut s2],
        );

        assert_eq!(outputs.len(), 2);
        // Service 0 (FakeService, source "fake") reacted to "bad";
        // FakeService2 only ever reports under source "fake2".
        assert_eq!(outputs[1]["params"]["diagnostics"][0]["source"], "fake");
    }

    /// (2c) The URI-extension match is case-insensitive: an uppercase
    /// extension still binds to the service that registered its
    /// lowercase form.
    #[test]
    fn uppercase_uri_extension_falls_back_to_the_extension_binding() {
        let mut s1 = FakeService::new();
        let mut s2 = FakeService2::new();
        let (outputs, _exit) = run_session_multi(
            &[
                initialize_message(1),
                // "plaintext" matches no service's languageId; ".F2"
                // matches FakeService2's ".f2" only case-insensitively.
                did_open_message_lang("file:///FOO.F2", "plaintext", 1, "boo"),
            ],
            &mut [&mut s1, &mut s2],
        );

        assert_eq!(outputs.len(), 2);
        assert_eq!(outputs[1]["params"]["diagnostics"][0]["source"], "fake2");
    }

    /// (3) completion/definition/formatting all follow the binding.
    #[test]
    fn feature_requests_follow_the_document_binding() {
        let mut s1 = FakeService::new();
        let mut s2 = FakeService2::new();
        let (outputs, _exit) = run_session_multi(
            &[
                initialize_message(1),
                did_open_message_lang("file:///b.f2", "fake2", 1, "x ref"),
                request_message(
                    2,
                    "textDocument/completion",
                    serde_json::json!({
                        "textDocument": {"uri": "file:///b.f2"},
                        "position": {"line": 0, "character": 0},
                    }),
                ),
                request_message(
                    3,
                    "textDocument/definition",
                    serde_json::json!({
                        "textDocument": {"uri": "file:///b.f2"},
                        "position": {"line": 0, "character": 0},
                    }),
                ),
                request_message(
                    4,
                    "textDocument/formatting",
                    serde_json::json!({"textDocument": {"uri": "file:///b.f2"}}),
                ),
            ],
            &mut [&mut s1, &mut s2],
        );

        assert_eq!(outputs.len(), 5);
        // completion → FakeService2's "beta".
        assert_eq!(outputs[2]["id"], serde_json::json!(2));
        assert_eq!(outputs[2]["result"][0]["label"], "beta");
        // definition → FakeService2 finds "ref" (cols 3..6 → 0-based 2..5).
        assert_eq!(outputs[3]["id"], serde_json::json!(3));
        assert_eq!(outputs[3]["result"]["uri"], "file:///b.f2");
        assert_eq!(outputs[3]["result"]["range"]["start"]["character"], 2);
        // formatting → FakeService2 uppercases the whole document.
        assert_eq!(outputs[4]["id"], serde_json::json!(4));
        assert_eq!(outputs[4]["result"][0]["newText"], "X REF");
    }

    /// (4) initialize merges trigger characters (dedup-union) and the
    /// semantic-token legend (types concatenated, modifiers dedup-unioned).
    #[test]
    fn initialize_merges_trigger_characters_and_the_token_legend() {
        let mut s1 = FakeService::new();
        let mut s2 = FakeService2::new();
        let (outputs, _exit) = run_session_multi(&[initialize_message(1)], &mut [&mut s1, &mut s2]);

        let caps = &outputs[0]["result"]["capabilities"];
        // '.' shared → deduped; '@' unique; first-seen order.
        assert_eq!(
            caps["completionProvider"]["triggerCharacters"],
            serde_json::json!([".", "@"])
        );
        // Types concatenated, NOT deduped — "function" appears twice.
        assert_eq!(
            caps["semanticTokensProvider"]["legend"]["tokenTypes"],
            serde_json::json!(["function", "kw", "function"])
        );
        // Modifiers dedup-unioned — "declaration" appears once.
        assert_eq!(
            caps["semanticTokensProvider"]["legend"]["tokenModifiers"],
            serde_json::json!(["declaration", "deprecated"])
        );
    }

    /// The watched-globs union rides `initialized`'s registerCapability.
    #[test]
    fn initialized_registers_the_union_of_watched_globs() {
        let mut s1 = FakeService::new();
        let mut s2 = FakeService2::new();
        let (outputs, _exit) = run_session_multi(
            &[
                initialize_message(1),
                serde_json::json!({"jsonrpc": "2.0", "method": "initialized", "params": {}}),
            ],
            &mut [&mut s1, &mut s2],
        );

        assert_eq!(outputs[1]["method"], "client/registerCapability");
        assert_eq!(
            outputs[1]["params"]["registrations"][0]["registerOptions"]["watchers"],
            serde_json::json!([{"globPattern": "**/fake.json"}, {"globPattern": "**/fake2.json"}])
        );
    }

    /// (5) A SemToken from service 1 arrives on the wire with its type
    /// index and modifier bits relocated into the merged legend.
    #[test]
    fn semantic_tokens_from_the_second_service_are_remapped_on_the_wire() {
        let mut s1 = FakeService::new();
        let mut s2 = FakeService2::new();
        let (outputs, _exit) = run_session_multi(
            &[
                initialize_message(1),
                did_open_message_lang("file:///c.f2", "fake2", 1, "kw"),
                request_message(
                    2,
                    "textDocument/semanticTokens/full",
                    serde_json::json!({"textDocument": {"uri": "file:///c.f2"}}),
                ),
            ],
            &mut [&mut s1, &mut s2],
        );

        assert_eq!(outputs[2]["id"], serde_json::json!(2));
        // Local (type 0 "kw", modifiers 0b1 "deprecated") relocates to
        // merged (type 0+offset₁=1, bit0→merged bit1 = 0b10=2). "kw" spans
        // cols 1..3 → packed [dLine, dStart, len, type, mods].
        assert_eq!(
            outputs[2]["result"]["data"],
            serde_json::json!([0, 0, 2, 1, 2])
        );
    }

    /// (5b) The reverse remap direction from (5): FakeService2's local
    /// bit 1 ("declaration") relocates DOWN to merged bit 0 — service
    /// 0's own "declaration" already claimed merged bit 0
    /// (`modifier_maps[1] == [1, 0]`, documented on `FakeService2`
    /// above), so service 2's HIGHER local bit collapses onto the
    /// LOWER merged bit. (5) only ever exercises the up-shift (local
    /// bit 0 -> merged bit 1); this pins the other direction.
    #[test]
    fn semantic_tokens_local_declaration_bit_remaps_down_to_merged_bit_zero() {
        let mut s1 = FakeService::new();
        let mut s2 = FakeService2::new();
        let (outputs, _exit) = run_session_multi(
            &[
                initialize_message(1),
                did_open_message_lang("file:///d.f2", "fake2", 1, "old"),
                request_message(
                    2,
                    "textDocument/semanticTokens/full",
                    serde_json::json!({"textDocument": {"uri": "file:///d.f2"}}),
                ),
            ],
            &mut [&mut s1, &mut s2],
        );

        assert_eq!(outputs[2]["id"], serde_json::json!(2));
        // Local (type 0 "kw", modifier bit 1 "declaration") relocates to
        // merged (type 0+offset₁=1, bit1 -> merged bit0 = 0b1=1). "old"
        // spans cols 1..4 -> packed [dLine, dStart, len, type, mods].
        assert_eq!(
            outputs[2]["result"]["data"],
            serde_json::json!([0, 0, 3, 1, 1])
        );
    }

    /// (6) didChangeConfiguration broadcasts to every service — both open
    /// documents republish with their own service's bumped revision.
    #[test]
    fn did_change_configuration_broadcasts_to_every_service() {
        let mut s1 = FakeService::new();
        let mut s2 = FakeService2::new();
        let (outputs, _exit) = run_session_multi(
            &[
                initialize_message(1),
                did_open_message_lang("file:///a.fake", "fake", 1, "bad"),
                did_open_message_lang("file:///b.f2", "fake2", 1, "boo"),
                notification_message(
                    "workspace/didChangeConfiguration",
                    serde_json::json!({"settings": {"n": 1}}),
                ),
            ],
            &mut [&mut s1, &mut s2],
        );

        // init, publish(a rev0), publish(b rev0), then URI-sorted
        // republishes (a.fake before b.f2) with each service's rev 1.
        assert_eq!(outputs.len(), 5);
        assert_eq!(outputs[3]["params"]["uri"], "file:///a.fake");
        assert_eq!(
            outputs[3]["params"]["diagnostics"][0]["message"],
            "bad word (config rev 1)"
        );
        assert_eq!(outputs[4]["params"]["uri"], "file:///b.f2");
        assert_eq!(
            outputs[4]["params"]["diagnostics"][0]["message"],
            "boo (fake2 rev 1)"
        );
    }

    /// (7) didClose removes the binding — a later request on that URI is
    /// "not open" and routes to service 0, flipping "beta" back to "alpha".
    #[test]
    fn did_close_unbinds_and_later_requests_route_to_service_zero() {
        let mut s1 = FakeService::new();
        let mut s2 = FakeService2::new();
        let (outputs, _exit) = run_session_multi(
            &[
                initialize_message(1),
                did_open_message_lang("file:///b.f2", "fake2", 1, "hi"),
                request_message(
                    2,
                    "textDocument/completion",
                    serde_json::json!({
                        "textDocument": {"uri": "file:///b.f2"},
                        "position": {"line": 0, "character": 0},
                    }),
                ),
                did_close_message("file:///b.f2"),
                request_message(
                    3,
                    "textDocument/completion",
                    serde_json::json!({
                        "textDocument": {"uri": "file:///b.f2"},
                        "position": {"line": 0, "character": 0},
                    }),
                ),
            ],
            &mut [&mut s1, &mut s2],
        );

        // While bound → FakeService2's "beta"; after close → service 0's
        // "alpha".
        assert_eq!(outputs[2]["id"], serde_json::json!(2));
        assert_eq!(outputs[2]["result"][0]["label"], "beta");
        assert_eq!(outputs[4]["id"], serde_json::json!(3));
        assert_eq!(outputs[4]["result"][0]["label"], "alpha");
    }

    /// (8) A single-service wrap is byte-identical to a dedicated server:
    /// identity merge maps leave the legend un-merged and the semantic
    /// tokens' wire bytes unchanged.
    #[test]
    fn single_service_capabilities_are_byte_identical_to_a_dedicated_server() {
        let mut service = FakeService::new();
        let (outputs, _exit) = run_session_multi(
            &[
                initialize_message(1),
                did_open_message("file:///a.fake", 1, "fn one\nfn two"),
                request_message(
                    2,
                    "textDocument/semanticTokens/full",
                    serde_json::json!({"textDocument": {"uri": "file:///a.fake"}}),
                ),
            ],
            &mut [&mut service],
        );

        let caps = &outputs[0]["result"]["capabilities"];
        assert_eq!(
            caps["completionProvider"]["triggerCharacters"],
            serde_json::json!(["."])
        );
        assert_eq!(
            caps["semanticTokensProvider"]["legend"]["tokenTypes"],
            serde_json::json!(["function"])
        );
        assert_eq!(
            caps["semanticTokensProvider"]["legend"]["tokenModifiers"],
            serde_json::json!(["declaration"])
        );
        // Identical to the pre-change `semantic_tokens_returns_packed_data_
        // for_two_fn_occurrences` expectation.
        assert_eq!(
            outputs[2]["result"]["data"],
            serde_json::json!([0, 0, 2, 0, 1, 1, 0, 2, 0, 1])
        );
    }
}
