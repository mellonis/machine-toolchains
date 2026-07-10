//! LSP 3.17 protocol structs (bounded subset). Pure serde data, no logic —
//! consumed by the server loop (docs/lsp.md) to build capabilities, publish
//! diagnostics, and decode feature-request params. No dead protocol
//! surface: only what the advertised capabilities need is modeled here.

use std::collections::HashMap;

/// A zero-based line/column position; columns are UTF-16 code units per
/// the LSP spec.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Position {
    pub line: u32,
    pub character: u32,
}

/// A half-open `[start, end)` span within a document.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Range {
    pub start: Position,
    pub end: Position,
}

/// A range within a specific document, identified by URI.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Location {
    pub uri: String,
    pub range: Range,
}

/// A textual edit applicable to a range. `new_text` is `newText` on the
/// wire.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextEdit {
    pub range: Range,
    pub new_text: String,
}

/// A set of text edits to apply, keyed by document URI.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceEdit {
    pub changes: HashMap<String, Vec<TextEdit>>,
}

/// A wire-shaped diagnostic; named to avoid clashing with
/// `diagnostics::Diagnostic`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WireDiagnostic {
    pub range: Range,
    /// 1 = Error, 2 = Warning (see `diagnostic_severity`).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub severity: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub source: Option<String>,
    pub message: String,
}

/// The legend for semantic-token indices: parallel index-to-name tables
/// for token types and modifiers.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticTokensLegend {
    pub token_types: Vec<String>,
    pub token_modifiers: Vec<String>,
}

/// `textDocumentSync` capability options: full-document sync, open/close
/// notifications on.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentSyncOptions {
    pub open_close: bool,
    pub change: u32,
}

/// `completionProvider` capability options.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionOptions {
    pub trigger_characters: Vec<String>,
}

/// `codeActionProvider` capability options.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeActionOptions {
    pub code_action_kinds: Vec<String>,
}

/// `semanticTokensProvider` capability options.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticTokensOptions {
    pub legend: SemanticTokensLegend,
    pub full: bool,
}

/// The full advertised server capability set (`initialize` result).
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerCapabilities {
    /// "utf-16".
    pub position_encoding: String,
    pub text_document_sync: TextDocumentSyncOptions,
    pub completion_provider: CompletionOptions,
    pub definition_provider: bool,
    pub document_formatting_provider: bool,
    pub document_symbol_provider: bool,
    pub code_action_provider: CodeActionOptions,
    pub semantic_tokens_provider: SemanticTokensOptions,
}

/// A symbol in the `textDocument/documentSymbol` outline tree.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSymbol {
    pub name: String,
    /// See `symbol_kind`.
    pub kind: u32,
    pub range: Range,
    pub selection_range: Range,
    pub children: Vec<DocumentSymbol>,
}

/// One `textDocument/completion` candidate.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletionItem {
    pub label: String,
    /// See `completion_item_kind`.
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub kind: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub text_edit: Option<TextEdit>,
}

/// One `textDocument/codeAction` candidate.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeAction {
    pub title: String,
    /// "quickfix".
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub is_preferred: Option<bool>,
    pub edit: WorkspaceEdit,
}

/// Identifies a document by URI, without a version.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentIdentifier {
    pub uri: String,
}

/// Identifies a document by URI at a specific version.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VersionedTextDocumentIdentifier {
    pub uri: String,
    pub version: i32,
}

/// A full document as sent on `textDocument/didOpen`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentItem {
    pub uri: String,
    pub language_id: String,
    pub version: i32,
    pub text: String,
}

/// `{ textDocument, position }` params shared by several requests.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentPositionParams {
    pub text_document: TextDocumentIdentifier,
    pub position: Position,
}

/// Params for `textDocument/didOpen`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidOpenTextDocumentParams {
    pub text_document: TextDocumentItem,
}

/// One entry in `DidChangeTextDocumentParams::content_changes`. Full sync
/// only — no `range` field, the whole document text is replaced.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TextDocumentContentChangeEvent {
    pub text: String,
}

/// Params for `textDocument/didChange`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidChangeTextDocumentParams {
    pub text_document: VersionedTextDocumentIdentifier,
    pub content_changes: Vec<TextDocumentContentChangeEvent>,
}

/// Params for `textDocument/didClose`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidCloseTextDocumentParams {
    pub text_document: TextDocumentIdentifier,
}

/// Params for the `textDocument/publishDiagnostics` notification.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PublishDiagnosticsParams {
    pub uri: String,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub version: Option<i32>,
    pub diagnostics: Vec<WireDiagnostic>,
}

/// Params for the `initialize` request. Client capabilities are
/// deliberately unread — we never branch on them in v1.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub initialization_options: Option<serde_json::Value>,
}

/// `serverInfo` on the `initialize` result.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfoWire {
    pub name: String,
    pub version: String,
}

/// Result of the `initialize` request.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    pub capabilities: ServerCapabilities,
    pub server_info: ServerInfoWire,
}

/// Params for `textDocument/codeAction`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CodeActionParams {
    pub text_document: TextDocumentIdentifier,
    pub range: Range,
}

/// Params for `textDocument/documentSymbol`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentSymbolParams {
    pub text_document: TextDocumentIdentifier,
}

/// Params for `textDocument/semanticTokens/full`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticTokensParams {
    pub text_document: TextDocumentIdentifier,
}

/// Result of `textDocument/semanticTokens/full`: the delta-encoded token
/// data array.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SemanticTokens {
    pub data: Vec<u32>,
}

/// Params for `textDocument/formatting`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DocumentFormattingParams {
    pub text_document: TextDocumentIdentifier,
}

/// Params for the `workspace/didChangeConfiguration` notification.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidChangeConfigurationParams {
    pub settings: serde_json::Value,
}

/// One entry in `DidChangeWatchedFilesParams::changes`.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEvent {
    pub uri: String,
    /// 1 = Created, 2 = Changed, 3 = Deleted. `"type"` on the wire.
    #[serde(rename = "type")]
    pub typ: u32,
}

/// Params for the `workspace/didChangeWatchedFiles` notification.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidChangeWatchedFilesParams {
    pub changes: Vec<FileEvent>,
}

/// One dynamic capability registration.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Registration {
    pub id: String,
    pub method: String,
    pub register_options: serde_json::Value,
}

/// Params for the server-initiated `client/registerCapability` request.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RegistrationParams {
    pub registrations: Vec<Registration>,
}

/// One filesystem glob watcher entry.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSystemWatcher {
    pub glob_pattern: String,
}

/// `registerOptions` shape for a `workspace/didChangeWatchedFiles`
/// registration.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DidChangeWatchedFilesRegistrationOptions {
    pub watchers: Vec<FileSystemWatcher>,
}

/// `Diagnostic.severity` values (LSP 3.17).
pub mod diagnostic_severity {
    pub const ERROR: u32 = 1;
    pub const WARNING: u32 = 2;
}

/// `CompletionItem.kind` values (LSP 3.17), bounded to what the server
/// emits.
pub mod completion_item_kind {
    pub const FUNCTION: u32 = 3;
    pub const MODULE: u32 = 9;
    pub const VALUE: u32 = 12;
    pub const KEYWORD: u32 = 14;
}

/// `DocumentSymbol.kind` values (LSP 3.17), bounded to what the server
/// emits.
pub mod symbol_kind {
    pub const NAMESPACE: u32 = 3;
    pub const FUNCTION: u32 = 12;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn server_capabilities_serializes_to_expected_json() {
        let caps = ServerCapabilities {
            position_encoding: "utf-16".to_string(),
            text_document_sync: TextDocumentSyncOptions {
                open_close: true,
                change: 1,
            },
            completion_provider: CompletionOptions {
                trigger_characters: vec!["@".to_string()],
            },
            definition_provider: true,
            document_formatting_provider: true,
            document_symbol_provider: true,
            code_action_provider: CodeActionOptions {
                code_action_kinds: vec!["quickfix".to_string()],
            },
            semantic_tokens_provider: SemanticTokensOptions {
                legend: SemanticTokensLegend {
                    token_types: vec!["function".to_string(), "namespace".to_string()],
                    token_modifiers: vec!["declaration".to_string()],
                },
                full: true,
            },
        };

        let got = serde_json::to_value(&caps).unwrap();
        assert_eq!(
            got,
            json!({
                "positionEncoding": "utf-16",
                "textDocumentSync": {"openClose": true, "change": 1},
                "completionProvider": {"triggerCharacters": ["@"]},
                "definitionProvider": true,
                "documentFormattingProvider": true,
                "documentSymbolProvider": true,
                "codeActionProvider": {"codeActionKinds": ["quickfix"]},
                "semanticTokensProvider": {
                    "legend": {
                        "tokenTypes": ["function", "namespace"],
                        "tokenModifiers": ["declaration"],
                    },
                    "full": true,
                },
            })
        );
    }

    #[test]
    fn did_change_text_document_params_deserializes_ignoring_unknown_fields() {
        let payload = json!({
            "textDocument": {"uri": "file:///a.fake", "version": 3, "extra": "ignored"},
            "contentChanges": [{"text": "new body"}],
            "somethingClientSpecific": {"nested": true},
        });

        let got: DidChangeTextDocumentParams = serde_json::from_value(payload).unwrap();
        assert_eq!(
            got,
            DidChangeTextDocumentParams {
                text_document: VersionedTextDocumentIdentifier {
                    uri: "file:///a.fake".to_string(),
                    version: 3,
                },
                content_changes: vec![TextDocumentContentChangeEvent {
                    text: "new body".to_string(),
                }],
            }
        );
    }

    #[test]
    fn file_event_round_trips_type_field() {
        let event = FileEvent {
            uri: "file:///a.fake".to_string(),
            typ: 2,
        };

        let value = serde_json::to_value(&event).unwrap();
        assert_eq!(value, json!({"uri": "file:///a.fake", "type": 2}));

        let back: FileEvent = serde_json::from_value(value).unwrap();
        assert_eq!(back, event);
    }

    #[test]
    fn wire_diagnostic_with_none_severity_omits_the_key() {
        let diag = WireDiagnostic {
            range: Range {
                start: Position {
                    line: 0,
                    character: 0,
                },
                end: Position {
                    line: 0,
                    character: 1,
                },
            },
            severity: None,
            code: None,
            source: None,
            message: "oops".to_string(),
        };

        let got = serde_json::to_value(&diag).unwrap();
        assert_eq!(
            got,
            json!({
                "range": {
                    "start": {"line": 0, "character": 0},
                    "end": {"line": 0, "character": 1},
                },
                "message": "oops",
            })
        );
    }

    #[test]
    fn text_edit_serializes_new_text() {
        let edit = TextEdit {
            range: Range {
                start: Position {
                    line: 1,
                    character: 2,
                },
                end: Position {
                    line: 1,
                    character: 5,
                },
            },
            new_text: "foo".to_string(),
        };

        let got = serde_json::to_value(&edit).unwrap();
        assert_eq!(
            got,
            json!({
                "range": {
                    "start": {"line": 1, "character": 2},
                    "end": {"line": 1, "character": 5},
                },
                "newText": "foo",
            })
        );
    }
}
