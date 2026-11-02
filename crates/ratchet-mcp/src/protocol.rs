//! JSON-RPC 2.0 framing and the MCP version handshake. One JSON object per line on stdio, as in the MCP spec's stdio transport.

use serde::Serialize;
use serde_json::{Value, json};

/// Protocol versions this server speaks, newest first.
pub const SUPPORTED_VERSIONS: &[&str] = &["2025-06-18", "2025-03-26", "2024-11-05"];

/// The version to answer `initialize` with: the client's own when we speak it, else our newest (the client then decides whether
/// it can talk to us, per the spec).
pub fn negotiate(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|r| SUPPORTED_VERSIONS.iter().find(|v| **v == r))
        .copied()
        .unwrap_or(SUPPORTED_VERSIONS[0])
}

pub const PARSE_ERROR: i64 = -32700;
pub const INVALID_REQUEST: i64 = -32600;
pub const METHOD_NOT_FOUND: i64 = -32601;
pub const INVALID_PARAMS: i64 = -32602;
pub const INTERNAL_ERROR: i64 = -32603;
/// The request needs `initialize` first.
pub const NOT_INITIALIZED: i64 = -32002;

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RpcError {
    pub code: i64,
    pub message: String,
}

impl RpcError {
    pub fn new(code: i64, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

pub fn result_message(id: &Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

pub fn error_message(id: &Value, e: &RpcError) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": e.code, "message": e.message}})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_version_is_echoed_and_an_unknown_one_gets_ours() {
        assert_eq!(negotiate(Some("2024-11-05")), "2024-11-05");
        assert_eq!(negotiate(Some("2025-03-26")), "2025-03-26");
        assert_eq!(negotiate(Some("2099-01-01")), "2025-06-18");
        assert_eq!(negotiate(None), "2025-06-18");
    }
}
