//! Small helpers for JSON-RPC 2.0 messages as used by MCP.

use serde_json::{Value, json};

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
/// The pooled server is starting, restarting or has failed.
pub const SERVER_UNAVAILABLE: i64 = -32000;

#[derive(Debug, PartialEq)]
pub enum Kind<'a> {
    Request { id: &'a Value, method: &'a str },
    Notification { method: &'a str },
    Response { id: &'a Value },
    Invalid,
}

pub fn classify(msg: &Value) -> Kind<'_> {
    let Some(obj) = msg.as_object() else { return Kind::Invalid };
    let method = obj.get("method").and_then(Value::as_str);
    let id = obj.get("id").filter(|v| !v.is_null());
    match (method, id) {
        (Some(method), Some(id)) => Kind::Request { id, method },
        (Some(method), None) => Kind::Notification { method },
        (None, Some(id)) if obj.contains_key("result") || obj.contains_key("error") => Kind::Response { id },
        _ => Kind::Invalid,
    }
}

pub fn error(id: Value, code: i64, message: impl Into<String>) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message.into()}})
}

pub fn result(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

pub fn notification(method: &str, params: Option<Value>) -> Value {
    match params {
        Some(p) => json!({"jsonrpc": "2.0", "method": method, "params": p}),
        None => json!({"jsonrpc": "2.0", "method": method}),
    }
}

pub fn request(id: Value, method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
}

/// Canonical text form of an id, for use as a map key. `1` and `"1"` differ.
pub fn id_key(id: &Value) -> String {
    id.to_string()
}

/// Serializes a message as one line of newline-delimited JSON.
pub fn to_line(msg: &Value) -> String {
    let mut s = serde_json::to_string(msg).unwrap_or_else(|_| "{}".into());
    s.push('\n');
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_messages() {
        let req = json!({"jsonrpc":"2.0","id":1,"method":"tools/list"});
        assert!(matches!(classify(&req), Kind::Request { method: "tools/list", .. }));
        let note = json!({"jsonrpc":"2.0","method":"notifications/initialized"});
        assert_eq!(classify(&note), Kind::Notification { method: "notifications/initialized" });
        let resp = json!({"jsonrpc":"2.0","id":"a","result":{}});
        assert!(matches!(classify(&resp), Kind::Response { .. }));
        let err = json!({"jsonrpc":"2.0","id":2,"error":{"code":1,"message":"x"}});
        assert!(matches!(classify(&err), Kind::Response { .. }));
        assert_eq!(classify(&json!({"jsonrpc":"2.0","id":3})), Kind::Invalid);
        assert_eq!(classify(&json!([1, 2])), Kind::Invalid);
        // A null id is a notification-shaped message, not a request.
        assert_eq!(classify(&json!({"method":"x","id":null})), Kind::Notification { method: "x" });
        // The ids 1 and "1" are different requests.
        assert_ne!(id_key(&json!(1)), id_key(&json!("1")));
    }
}
