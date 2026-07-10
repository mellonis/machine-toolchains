//! JSON-RPC 2.0 envelope decode/encode (LSP 3.17 base protocol). Sits above
//! `transport` (docs/lsp.md): `transport` frames a payload string, this
//! module interprets it as a JSON-RPC request/notification/response or
//! encodes an outgoing one. Doesn't validate the `jsonrpc: "2.0"` field
//! strictly — clients always send it, rejecting on it buys nothing.

use serde_json::Value;

/// Request/response id — number or string per JSON-RPC 2.0.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(untagged)]
pub enum Id {
    Number(i64),
    String(String),
}

/// One decoded incoming message.
#[derive(Debug, PartialEq)]
pub enum Message {
    Request {
        id: Id,
        method: String,
        params: Value,
    },
    Notification {
        method: String,
        params: Value,
    },
    /// A response to a server-initiated request; the loop drops these.
    Response {
        id: Option<Id>,
    },
}

/// Envelope decode failures.
#[derive(Debug, PartialEq, Eq)]
pub enum DecodeError {
    /// Not valid JSON at all → respond ParseError with null id.
    Json(String),
    /// Valid JSON but not a JSON-RPC 2.0 message → InvalidRequest.
    Shape(&'static str),
}

impl std::fmt::Display for DecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Json(msg) => write!(f, "invalid json: {msg}"),
            Self::Shape(reason) => write!(f, "not a json-rpc 2.0 message: {reason}"),
        }
    }
}

impl std::error::Error for DecodeError {}

/// Decodes one JSON-RPC payload. Must be a JSON object (arrays/batches →
/// `Shape`); `method`+`id` → `Request`; `method` only → `Notification`;
/// `id` (or `result`/`error`) without `method` → `Response`; absent
/// `params` decodes as `Value::Null`.
pub fn decode(payload: &str) -> Result<Message, DecodeError> {
    let value: Value =
        serde_json::from_str(payload).map_err(|e| DecodeError::Json(e.to_string()))?;
    let object = value
        .as_object()
        .ok_or(DecodeError::Shape("payload is not a json object"))?;

    let method = match object.get("method") {
        Some(Value::String(method)) => Some(method.clone()),
        Some(_) => return Err(DecodeError::Shape("method is not a string")),
        None => None,
    };
    let params = object.get("params").cloned().unwrap_or(Value::Null);
    let id = match object.get("id") {
        Some(Value::Number(n)) => {
            let n = n
                .as_i64()
                .ok_or(DecodeError::Shape("id number is out of i64 range"))?;
            Some(Id::Number(n))
        }
        Some(Value::String(s)) => Some(Id::String(s.clone())),
        Some(Value::Null) | None => None,
        Some(_) => return Err(DecodeError::Shape("id is not a number or string")),
    };

    match (method, id) {
        (Some(method), Some(id)) => Ok(Message::Request { id, method, params }),
        (Some(method), None) => Ok(Message::Notification { method, params }),
        (None, id) => {
            if id.is_some() || object.contains_key("result") || object.contains_key("error") {
                Ok(Message::Response { id })
            } else {
                Err(DecodeError::Shape(
                    "neither a request, notification, nor response",
                ))
            }
        }
    }
}

/// Encodes a successful response.
pub fn response_ok(id: &Id, result: Value) -> String {
    serde_json::to_string(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": result,
    }))
    .expect("response_ok payload is always serializable")
}

/// Encodes an error response. `id: None` → `"id": null`.
pub fn response_err(id: Option<&Id>, code: i64, message: &str) -> String {
    serde_json::to_string(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "error": {
            "code": code,
            "message": message,
        },
    }))
    .expect("response_err payload is always serializable")
}

/// Encodes an outgoing notification.
pub fn notification(method: &str, params: Value) -> String {
    serde_json::to_string(&serde_json::json!({
        "jsonrpc": "2.0",
        "method": method,
        "params": params,
    }))
    .expect("notification payload is always serializable")
}

/// Encodes an outgoing server-initiated request.
pub fn request(id: i64, method: &str, params: Value) -> String {
    serde_json::to_string(&serde_json::json!({
        "jsonrpc": "2.0",
        "id": id,
        "method": method,
        "params": params,
    }))
    .expect("request payload is always serializable")
}

/// JSON-RPC / LSP well-known error codes.
pub mod error_codes {
    pub const PARSE_ERROR: i64 = -32700;
    pub const INVALID_REQUEST: i64 = -32600;
    pub const METHOD_NOT_FOUND: i64 = -32601;
    pub const INVALID_PARAMS: i64 = -32602;
    pub const INTERNAL_ERROR: i64 = -32603;
    pub const SERVER_NOT_INITIALIZED: i64 = -32002;
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn decodes_request_with_number_id() {
        let payload = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"x":1}}"#;
        assert_eq!(
            decode(payload).unwrap(),
            Message::Request {
                id: Id::Number(1),
                method: "initialize".to_string(),
                params: json!({"x": 1}),
            }
        );
    }

    #[test]
    fn decodes_request_with_string_id() {
        let payload = r#"{"jsonrpc":"2.0","id":"req-1","method":"initialize","params":{}}"#;
        assert_eq!(
            decode(payload).unwrap(),
            Message::Request {
                id: Id::String("req-1".to_string()),
                method: "initialize".to_string(),
                params: json!({}),
            }
        );
    }

    #[test]
    fn decodes_notification() {
        let payload = r#"{"jsonrpc":"2.0","method":"initialized","params":{}}"#;
        assert_eq!(
            decode(payload).unwrap(),
            Message::Notification {
                method: "initialized".to_string(),
                params: json!({}),
            }
        );
    }

    #[test]
    fn missing_params_decodes_as_null() {
        let payload = r#"{"jsonrpc":"2.0","id":1,"method":"shutdown"}"#;
        assert_eq!(
            decode(payload).unwrap(),
            Message::Request {
                id: Id::Number(1),
                method: "shutdown".to_string(),
                params: Value::Null,
            }
        );
    }

    #[test]
    fn decodes_result_response() {
        let payload = r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
        assert_eq!(
            decode(payload).unwrap(),
            Message::Response {
                id: Some(Id::Number(1))
            }
        );
    }

    #[test]
    fn decodes_error_response() {
        let payload = r#"{"jsonrpc":"2.0","id":"req-1","error":{"code":-32600,"message":"bad"}}"#;
        assert_eq!(
            decode(payload).unwrap(),
            Message::Response {
                id: Some(Id::String("req-1".to_string()))
            }
        );
    }

    #[test]
    fn malformed_json_is_json_decode_error() {
        let err = decode("not json").unwrap_err();
        assert!(matches!(err, DecodeError::Json(_)));
    }

    #[test]
    fn json_array_is_shape_decode_error() {
        let err = decode("[1,2,3]").unwrap_err();
        assert!(matches!(err, DecodeError::Shape(_)));
    }

    #[test]
    fn response_ok_emits_expected_json() {
        let payload = response_ok(&Id::Number(1), json!({"foo": "bar"}));
        let got: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(
            got,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "result": {"foo": "bar"},
            })
        );
    }

    #[test]
    fn response_ok_emits_expected_json_with_string_id() {
        let payload = response_ok(&Id::String("req-1".to_string()), json!(null));
        let got: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(
            got,
            json!({
                "jsonrpc": "2.0",
                "id": "req-1",
                "result": null,
            })
        );
    }

    #[test]
    fn response_err_emits_expected_json() {
        let payload = response_err(
            Some(&Id::Number(7)),
            error_codes::INVALID_PARAMS,
            "bad params",
        );
        let got: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(
            got,
            json!({
                "jsonrpc": "2.0",
                "id": 7,
                "error": {
                    "code": -32602,
                    "message": "bad params",
                },
            })
        );
    }

    #[test]
    fn response_err_with_none_id_carries_null_id() {
        let payload = response_err(None, error_codes::PARSE_ERROR, "parse error");
        let got: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(
            got,
            json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {
                    "code": -32700,
                    "message": "parse error",
                },
            })
        );
    }

    #[test]
    fn notification_emits_expected_json() {
        let payload = notification("window/logMessage", json!({"type": 3, "message": "hi"}));
        let got: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(
            got,
            json!({
                "jsonrpc": "2.0",
                "method": "window/logMessage",
                "params": {"type": 3, "message": "hi"},
            })
        );
    }

    #[test]
    fn request_emits_expected_json() {
        let payload = request(1, "workspace/configuration", json!({"items": []}));
        let got: Value = serde_json::from_str(&payload).unwrap();
        assert_eq!(
            got,
            json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": "workspace/configuration",
                "params": {"items": []},
            })
        );
    }
}
