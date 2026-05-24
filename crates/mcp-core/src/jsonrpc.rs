//! JSON-RPC 2.0 message framing.

use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const JSONRPC_VERSION: &str = "2.0";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Id {
    Number(i64),
    String(String),
    Null,
}

impl From<i64> for Id {
    fn from(n: i64) -> Self { Self::Number(n) }
}

impl From<String> for Id {
    fn from(s: String) -> Self { Self::String(s) }
}

impl From<&str> for Id {
    fn from(s: &str) -> Self { Self::String(s.to_string()) }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub jsonrpc: String,
    pub id: Id,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl Request {
    pub fn new(id: impl Into<Id>, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            id: id.into(),
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Notification {
    pub jsonrpc: String,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl Notification {
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: JSONRPC_VERSION.to_string(),
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub jsonrpc: String,
    pub id: Id,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorObject>,
}

impl Response {
    pub fn success(id: Id, result: Value) -> Self {
        Self { jsonrpc: JSONRPC_VERSION.to_string(), id, result: Some(result), error: None }
    }

    pub fn failure(id: Id, error: ErrorObject) -> Self {
        Self { jsonrpc: JSONRPC_VERSION.to_string(), id, result: None, error: Some(error) }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, thiserror::Error)]
#[error("[{code}] {message}")]
pub struct ErrorObject {
    pub code: i32,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl ErrorObject {
    pub fn new(code: i32, message: impl Into<String>) -> Self {
        Self { code, message: message.into(), data: None }
    }

    pub fn with_data(mut self, data: Value) -> Self {
        self.data = Some(data);
        self
    }
}

/// A JSON-RPC 2.0 message: request, response, or notification.
///
/// Distinguishes by the presence of `id` (request/response) and `method`
/// (request/notification).
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Message {
    Request(Request),
    Response(Response),
    Notification(Notification),
}

impl Message {
    pub fn from_json(bytes: &[u8]) -> Result<Self, serde_json::Error> {
        // The untagged enum is ambiguous between Request and Response when both `id`
        // and `result`/`error` could coexist; route by inspecting the parsed value.
        let value: Value = serde_json::from_slice(bytes)?;
        Self::from_value(value)
    }

    pub fn from_value(value: Value) -> Result<Self, serde_json::Error> {
        let has_method = value.get("method").is_some();
        let has_id = value.get("id").is_some();
        if has_method && has_id {
            Ok(Message::Request(serde_json::from_value(value)?))
        } else if has_method {
            Ok(Message::Notification(serde_json::from_value(value)?))
        } else {
            Ok(Message::Response(serde_json::from_value(value)?))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip_request() {
        let req = Request::new(1i64, "initialize", Some(serde_json::json!({"x": 1})));
        let s = serde_json::to_string(&req).unwrap();
        let m = Message::from_json(s.as_bytes()).unwrap();
        match m {
            Message::Request(r) => {
                assert_eq!(r.method, "initialize");
                assert_eq!(r.id, Id::Number(1));
            }
            _ => panic!("expected request"),
        }
    }

    #[test]
    fn parse_notification() {
        let n = serde_json::json!({"jsonrpc": "2.0", "method": "notifications/progress"});
        let m = Message::from_value(n).unwrap();
        assert!(matches!(m, Message::Notification(_)));
    }

    #[test]
    fn parse_response() {
        let r = serde_json::json!({"jsonrpc": "2.0", "id": 1, "result": {"ok": true}});
        let m = Message::from_value(r).unwrap();
        assert!(matches!(m, Message::Response(_)));
    }
}
