//! Blocking LSP server loop (docs/lsp.md "Runtime model"): reads
//! Content-Length-framed JSON-RPC messages off `reader`, dispatches them
//! against a [`LanguageService`], and enforces the LSP lifecycle —
//! initialize/initialized/shutdown/exit gating, unknown-method handling,
//! and decode-error responses. Document sync (didOpen/didChange/
//! didClose/publish) and feature dispatch (completion, definition, …)
//! are layered onto the same `dispatch` seam by later tasks; this task
//! implements lifecycle only.

use super::LanguageService;
use super::docstore::DocStore;
use super::jsonrpc::{self, DecodeError, Id, Message, error_codes};
use super::transport;
use super::types::{
    CodeActionOptions, CompletionOptions, DidChangeWatchedFilesRegistrationOptions,
    FileSystemWatcher, InitializeParams, InitializeResult, Registration, RegistrationParams,
    SemanticTokensLegend, SemanticTokensOptions, ServerCapabilities, ServerInfoWire,
    TextDocumentSyncOptions,
};

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
    /// Wired in Task 8 (document sync + diagnostics publishing); this
    /// task only constructs it.
    #[allow(dead_code)]
    docs: DocStore,
}

impl ServerState {
    fn new() -> Self {
        ServerState {
            initialized: false,
            shutdown: false,
            next_request_id: 1,
            docs: DocStore::new(),
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
fn dispatch(
    state: &mut ServerState,
    writer: &mut dyn std::io::Write,
    service: &mut dyn LanguageService,
    identity: ServerIdentity,
    message: Message,
) -> Signal {
    match message {
        Message::Request { id, method, params } => {
            handle_request(state, writer, service, identity, &id, &method, params);
            Signal::Continue
        }
        Message::Notification { method, params } => {
            handle_notification(state, writer, service, &method, params)
        }
        Message::Response { .. } => Signal::Continue,
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
    _params: serde_json::Value,
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
        // Unknown notifications (including every `$/…` method, e.g.
        // `$/cancelRequest`) and not-yet-implemented ones (didOpen/
        // didChange/… land in Task 8) are silently dropped per spec.
        _ => Signal::Continue,
    }
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
}
