//! Blocking LSP server loop (docs/lsp.md "Runtime model"): reads
//! Content-Length-framed JSON-RPC messages off `reader`, dispatches them
//! against a [`LanguageService`], and enforces the LSP lifecycle —
//! initialize/initialized/shutdown/exit gating, unknown-method handling,
//! and decode-error responses. Document sync (didOpen/didChange/
//! didClose) drives the `DocStore` and republishes diagnostics through
//! one `publish` helper; the same helper powers the config- and
//! watched-file-triggered republish-all sweeps. Feature requests
//! (completion, definition, code actions, document symbols, semantic
//! tokens, formatting) convert trait output to wire types via
//! `position`. Every dispatched message runs under `catch_unwind` so a
//! panicking handler can't take the whole session down (docs/lsp.md
//! "Error containment").

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
    FileSystemWatcher, InitializeParams, InitializeResult, Location, LocationLink, Position,
    PublishDiagnosticsParams, Range, Registration, RegistrationParams, SemanticTokens,
    SemanticTokensLegend, SemanticTokensOptions, SemanticTokensParams, ServerCapabilities,
    ServerInfoWire, TextDocumentPositionParams, TextDocumentSyncOptions, TextEdit, WireDiagnostic,
    WorkspaceEdit, completion_item_kind, diagnostic_severity, symbol_kind,
};
use super::{
    Action, Candidate, CandidateKind, DefTarget, ServiceDiagnostic, ServiceSeverity, SymbolNode,
    SymbolNodeKind,
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
}

impl ServerState {
    fn new() -> Self {
        ServerState {
            initialized: false,
            shutdown: false,
            next_request_id: 1,
            docs: DocStore::new(),
            definition_link_support: false,
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
pub fn run(
    reader: &mut dyn std::io::BufRead,
    writer: &mut dyn std::io::Write,
    service: &mut dyn LanguageService,
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

        match dispatch(&mut state, writer, service, identity, message) {
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
    service: &mut dyn LanguageService,
    identity: ServerIdentity,
    message: Message,
) -> Signal {
    match message {
        Message::Request { id, method, params } => {
            let outcome = {
                let _suppress = SuppressPanicOutput::new();
                panic::catch_unwind(AssertUnwindSafe(|| {
                    handle_request(state, writer, service, identity, &id, &method, params);
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
                    handle_notification(state, writer, service, &method, params)
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
    service: &mut dyn LanguageService,
    identity: ServerIdentity,
    id: &Id,
    method: &str,
    params: serde_json::Value,
) {
    match method {
        "initialize" => handle_initialize(state, writer, service, identity, id, params),
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
        "textDocument/completion" => handle_completion(state, writer, service, id, params),
        "textDocument/definition" => handle_definition(state, writer, service, id, params),
        "textDocument/codeAction" => handle_code_action(state, writer, service, id, params),
        "textDocument/documentSymbol" => handle_document_symbol(state, writer, service, id, params),
        "textDocument/semanticTokens/full" => {
            handle_semantic_tokens(state, writer, service, id, params)
        }
        "textDocument/formatting" => handle_formatting(state, writer, service, id, params),
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
    service: &mut dyn LanguageService,
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
    if let Some(options) = init_params.initialization_options {
        service.did_change_config(options);
    }

    state.initialized = true;

    let result = build_initialize_result(service, identity);
    let result_value =
        serde_json::to_value(result).expect("InitializeResult is always serializable");
    respond_ok(writer, id, result_value);
}

/// Notifications carry no response; the return value is the loop
/// signal (only `exit` produces `Signal::Exit`).
fn handle_notification(
    state: &mut ServerState,
    writer: &mut dyn std::io::Write,
    service: &mut dyn LanguageService,
    method: &str,
    params: serde_json::Value,
) -> Signal {
    match method {
        "exit" => Signal::Exit,
        // Notifications before initialize are dropped — except `exit`,
        // handled above.
        _ if !state.initialized => Signal::Continue,
        "initialized" => {
            send_register_capability(state, writer, service);
            Signal::Continue
        }
        "textDocument/didOpen" => {
            if let Ok(params) = serde_json::from_value::<DidOpenTextDocumentParams>(params) {
                let uri = params.text_document.uri;
                state.docs.open(
                    &uri,
                    params.text_document.version,
                    params.text_document.text,
                );
                publish(state, writer, service, &uri);
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
                publish(state, writer, service, &uri);
            }
            Signal::Continue
        }
        "textDocument/didClose" => {
            if let Ok(params) = serde_json::from_value::<DidCloseTextDocumentParams>(params) {
                let uri = params.text_document.uri;
                state.docs.close(&uri);
                service.did_close(&uri);
                publish(state, writer, service, &uri);
            }
            Signal::Continue
        }
        "workspace/didChangeConfiguration" => {
            if let Ok(params) = serde_json::from_value::<DidChangeConfigurationParams>(params) {
                service.did_change_config(params.settings);
                republish_all(state, writer, service);
            }
            Signal::Continue
        }
        // No config parsing here — core stays language-agnostic; the
        // service re-reads its own config/watched sources during
        // `did_update`. This notification exists only to trigger the
        // same republish-all sweep.
        "workspace/didChangeWatchedFiles" => {
            if serde_json::from_value::<DidChangeWatchedFilesParams>(params).is_ok() {
                republish_all(state, writer, service);
            }
            Signal::Continue
        }
        // Unknown notifications (including every `$/…` method, e.g.
        // `$/cancelRequest`) are silently dropped per spec.
        _ => Signal::Continue,
    }
}

/// Publishes `uri`'s complete diagnostic set. The single path every
/// (re)publish goes through (didOpen/didChange above; Task 9's
/// config/watched-files republish sweeps reuse it directly by calling
/// it once per open document). Looks the document up in the store
/// itself: when open, re-runs `service.did_update` against its current
/// text and publishes with its version; when absent (post-didClose),
/// publishes an empty set with the version omitted.
fn publish(
    state: &ServerState,
    writer: &mut dyn std::io::Write,
    service: &mut dyn LanguageService,
    uri: &str,
) {
    let (version, diagnostics) = match state.docs.get(uri) {
        Some(doc) => {
            let diagnostics = service
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
    }
}

/// Re-publishes diagnostics for every open document, in URI-sorted
/// order — the sweep both `workspace/didChangeConfiguration` and
/// `workspace/didChangeWatchedFiles` trigger. Core has no opinion on
/// what changed; the service re-reads its own config/watched-file
/// sources from inside `did_update` itself.
fn republish_all(
    state: &ServerState,
    writer: &mut dyn std::io::Write,
    service: &mut dyn LanguageService,
) {
    for uri in state.docs.uris() {
        publish(state, writer, service, &uri);
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
    service: &mut dyn LanguageService,
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

    let items: Vec<CompletionItem> = service
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
        text_edit: Some(TextEdit {
            range: position::span_to_range(text, candidate.replace_span),
            new_text: candidate.insert_text,
        }),
    }
}

fn handle_definition(
    state: &ServerState,
    writer: &mut dyn std::io::Write,
    service: &mut dyn LanguageService,
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

    let result = match service.definition(&uri, pos) {
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

fn handle_code_action(
    state: &ServerState,
    writer: &mut dyn std::io::Write,
    service: &mut dyn LanguageService,
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

    let actions: Vec<CodeAction> = service
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
    service: &mut dyn LanguageService,
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

    let result = match service.document_symbols(&uri) {
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
    service: &mut dyn LanguageService,
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

    let result = match service.semantic_tokens(&uri) {
        Some(tokens) => {
            let data = position::pack_semantic_tokens(text, &tokens);
            serde_json::to_value(SemanticTokens { data })
                .expect("SemanticTokens is always serializable")
        }
        None => serde_json::Value::Null,
    };
    respond_ok(writer, id, result);
}

fn handle_formatting(
    state: &ServerState,
    writer: &mut dyn std::io::Write,
    service: &mut dyn LanguageService,
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

    let result = match service.format(&uri) {
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
    service: &mut dyn LanguageService,
    identity: ServerIdentity,
) -> InitializeResult {
    let (token_types, token_modifiers) = service.token_legend();

    InitializeResult {
        capabilities: ServerCapabilities {
            position_encoding: "utf-16".to_string(),
            text_document_sync: TextDocumentSyncOptions {
                open_close: true,
                change: 1,
            },
            completion_provider: CompletionOptions {
                trigger_characters: service
                    .trigger_characters()
                    .iter()
                    .map(|c| c.to_string())
                    .collect(),
            },
            definition_provider: true,
            document_formatting_provider: true,
            document_symbol_provider: true,
            code_action_provider: CodeActionOptions {
                code_action_kinds: vec!["quickfix".to_string()],
            },
            semantic_tokens_provider: SemanticTokensOptions {
                legend: SemanticTokensLegend {
                    token_types: token_types.iter().map(|s| s.to_string()).collect(),
                    token_modifiers: token_modifiers.iter().map(|s| s.to_string()).collect(),
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
/// `workspace/didChangeWatchedFiles`, skipped entirely when the service
/// advertises no globs to watch.
fn send_register_capability(
    state: &mut ServerState,
    writer: &mut dyn std::io::Write,
    service: &mut dyn LanguageService,
) {
    let globs = service.watched_globs();
    if globs.is_empty() {
        return;
    }

    let id = state.next_request_id;
    state.next_request_id += 1;

    let watchers = globs
        .iter()
        .map(|glob| FileSystemWatcher {
            glob_pattern: (*glob).to_string(),
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
        let mut input = Vec::new();
        for msg in client_messages {
            transport::write_message(&mut input, &msg.to_string())
                .expect("write_message into a Vec cannot fail");
        }

        let mut output = Vec::new();
        let mut reader = &input[..];
        let exit_code = run(&mut reader, &mut output, service, TEST_IDENTITY);

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

        let exit_code = run(&mut reader, &mut output, &mut service, TEST_IDENTITY);

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
        let exit_code = run(&mut reader, &mut output, &mut service, TEST_IDENTITY);
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
        let exit_code = run(&mut reader, &mut output, &mut service, TEST_IDENTITY);
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
}
