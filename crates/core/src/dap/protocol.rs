//! DAP's base-protocol envelope: the `seq`-counted, `"type"`-tagged JSON
//! object every request, response, and event is wrapped in. Sits at the
//! same layer `lsp::jsonrpc` sits at for LSP — above the shared
//! Content-Length `framing` codec, below any per-command knowledge.
//! Unlike JSON-RPC's `method`+`params`, DAP's envelope carries the kind
//! as a flat `"type"` string alongside `"command"`/`"event"`, so the
//! three kinds are modeled as one internally tagged enum flattened onto
//! the outer `seq`. Per-command `arguments`/`body` shapes are
//! deliberately left as raw `serde_json::Value` here — this module knows
//! the envelope, not the ~20 DAP request/response payload schemas, so an
//! adapter can typecheck those on top without this layer needing to
//! recognize every command up front.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// One decoded (or to-be-encoded) DAP message: the running `seq` counter
/// plus its per-kind payload, flattened onto one JSON object.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProtocolMessage {
    pub seq: u64,
    #[serde(flatten)]
    pub kind: MessageKind,
}

/// The three DAP message kinds, internally tagged by `"type"` with the
/// variant name lowercased to match the wire values `"request"` /
/// `"response"` / `"event"`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum MessageKind {
    Request {
        command: String,
        /// Raw request payload. Defaults to `Value::Null` when the field
        /// is absent (several DAP requests, e.g. `threads`, carry none).
        #[serde(default)]
        arguments: Value,
    },
    Response {
        request_seq: u64,
        success: bool,
        command: String,
        /// Short failure description. Present on the wire only for
        /// failed responses; omitted entirely (not `null`) when absent.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        /// Raw response payload. Defaults to `Value::Null` when the
        /// field is absent (several DAP responses carry no body).
        #[serde(default)]
        body: Value,
    },
    Event {
        event: String,
        /// Raw event payload; see the `arguments`/`body` note above.
        #[serde(default)]
        body: Value,
    },
}

impl ProtocolMessage {
    /// Builds a response to the request numbered `request_seq`, itself
    /// numbered `seq`. `Ok(body)` encodes success with that body;
    /// `Err(message)` encodes failure (`success: false`, `message` set,
    /// body `Value::Null`).
    pub fn response_to(
        request_seq: u64,
        seq: u64,
        command: impl Into<String>,
        result: Result<Value, String>,
    ) -> Self {
        let (success, message, body) = match result {
            Ok(body) => (true, None, body),
            Err(message) => (false, Some(message), Value::Null),
        };
        ProtocolMessage {
            seq,
            kind: MessageKind::Response {
                request_seq,
                success,
                command: command.into(),
                message,
                body,
            },
        }
    }

    /// Builds an event numbered `seq`.
    pub fn event(seq: u64, name: impl Into<String>, body: Value) -> Self {
        ProtocolMessage {
            seq,
            kind: MessageKind::Event {
                event: name.into(),
                body,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// The `initialize` request example from the DAP walkthrough
    /// (Microsoft's mock-debug adapter sample exchange).
    const INITIALIZE_REQUEST: &str = r#"{
        "seq": 1,
        "type": "request",
        "command": "initialize",
        "arguments": {
            "clientID": "vscode",
            "clientName": "Visual Studio Code",
            "adapterID": "mock",
            "pathFormat": "path",
            "linesStartAt1": true,
            "columnsStartAt1": true,
            "supportsVariableType": true,
            "supportsVariablePaging": true,
            "supportsRunInTerminalRequest": true,
            "locale": "en-us"
        }
    }"#;

    #[test]
    fn decodes_a_literal_initialize_request() {
        let msg: ProtocolMessage = serde_json::from_str(INITIALIZE_REQUEST).unwrap();

        assert_eq!(msg.seq, 1);
        match msg.kind {
            MessageKind::Request { command, arguments } => {
                assert_eq!(command, "initialize");
                assert_eq!(arguments["clientID"], json!("vscode"));
                assert_eq!(arguments["adapterID"], json!("mock"));
                assert_eq!(arguments["linesStartAt1"], json!(true));
            }
            other => panic!("expected Request, got {other:?}"),
        }
    }

    #[test]
    fn encodes_a_successful_response_matching_the_dap_wire_shape() {
        let msg = ProtocolMessage::response_to(
            153,
            154,
            "initialize",
            Ok(json!({"supportsConfigurationDoneRequest": true})),
        );

        let value = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            value,
            json!({
                "seq": 154,
                "type": "response",
                "request_seq": 153,
                "success": true,
                "command": "initialize",
                "body": {"supportsConfigurationDoneRequest": true},
            })
        );
    }

    #[test]
    fn encodes_a_failed_response_with_message_and_no_body() {
        let msg =
            ProtocolMessage::response_to(9, 10, "evaluate", Err("unknown expression".to_string()));

        let value = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            value,
            json!({
                "seq": 10,
                "type": "response",
                "request_seq": 9,
                "success": false,
                "command": "evaluate",
                "message": "unknown expression",
                "body": null,
            })
        );
    }

    #[test]
    fn encodes_an_event_matching_the_dap_wire_shape() {
        let msg = ProtocolMessage::event(155, "stopped", json!({"reason": "step", "threadId": 3}));

        let value = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            value,
            json!({
                "seq": 155,
                "type": "event",
                "event": "stopped",
                "body": {"reason": "step", "threadId": 3},
            })
        );
    }

    #[test]
    fn round_trips_an_unknown_command_preserving_raw_arguments() {
        let raw = json!({
            "seq": 42,
            "type": "request",
            "command": "someFutureRequest",
            "arguments": {
                "nested": {"a": [1, 2, 3]},
                "flag": null,
                "note": "unrecognized commands still decode"
            }
        });

        let msg: ProtocolMessage = serde_json::from_value(raw.clone()).unwrap();
        match &msg.kind {
            MessageKind::Request { command, .. } => assert_eq!(command, "someFutureRequest"),
            other => panic!("expected Request, got {other:?}"),
        }

        let round_tripped = serde_json::to_value(&msg).unwrap();
        assert_eq!(round_tripped, raw);
    }

    #[test]
    fn decodes_a_response_without_a_body() {
        // the "next" response from the DAP overview walkthrough
        let msg: ProtocolMessage = serde_json::from_str(
            r#"{"seq":154,"type":"response","request_seq":153,"success":true,"command":"next"}"#,
        )
        .unwrap();

        assert_eq!(msg.seq, 154);
        match msg.kind {
            MessageKind::Response {
                request_seq,
                success,
                command,
                message,
                body,
            } => {
                assert_eq!(request_seq, 153);
                assert!(success);
                assert_eq!(command, "next");
                assert_eq!(message, None);
                assert_eq!(body, Value::Null);
            }
            other => panic!("expected Response, got {other:?}"),
        }
    }

    #[test]
    fn defaults_missing_arguments_and_body_to_null() {
        let msg: ProtocolMessage =
            serde_json::from_str(r#"{"seq": 7, "type": "request", "command": "threads"}"#).unwrap();

        match msg.kind {
            MessageKind::Request { arguments, .. } => assert_eq!(arguments, Value::Null),
            other => panic!("expected Request, got {other:?}"),
        }
    }
}
