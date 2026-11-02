//! The backend of `ratchetd mcp --session <token>`: every call is one `POST /v1/mcp/call` to the daemon on its unix socket,
//! carrying the session's capability token. The daemon decides what the token may do.

use std::path::PathBuf;

use async_trait::async_trait;
use http_body_util::{BodyExt, Full};
use hyper::Request;
use hyper::body::Bytes;
use hyper_util::rt::TokioIo;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::net::UnixStream;

use crate::sets::ToolSet;
use crate::tool::{ToolBackend, ToolError};

pub const CALL_PATH: &str = "/v1/mcp/call";
pub const SESSION_PATH: &str = "/v1/mcp/session";

/// What the daemon says about a session token.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDescription {
    pub id: String,
    pub set: ToolSet,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub optimization: Option<String>,
}

pub struct SocketBackend {
    socket: PathBuf,
    token: String,
}

impl SocketBackend {
    pub fn new(socket: impl Into<PathBuf>, token: impl Into<String>) -> Self {
        Self {
            socket: socket.into(),
            token: token.into(),
        }
    }

    async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<Vec<u8>>,
    ) -> Result<(u16, Bytes), String> {
        let stream = UnixStream::connect(&self.socket).await.map_err(|e| {
            format!(
                "cannot reach ratchetd at {}: {e} (is the daemon running?)",
                self.socket.display()
            )
        })?;
        let (mut sender, conn) = hyper::client::conn::http1::handshake(TokioIo::new(stream))
            .await
            .map_err(|e| e.to_string())?;
        tokio::spawn(conn);
        let req = Request::builder()
            .method(method)
            .uri(path)
            .header("host", "localhost")
            .header("authorization", format!("Bearer {}", self.token))
            .header("content-type", "application/json")
            .body(Full::new(Bytes::from(body.unwrap_or_default())))
            .map_err(|e| e.to_string())?;
        let resp = sender.send_request(req).await.map_err(|e| e.to_string())?;
        let (parts, body) = resp.into_parts();
        let bytes = body.collect().await.map_err(|e| e.to_string())?.to_bytes();
        Ok((parts.status.as_u16(), bytes))
    }
}

impl SocketBackend {
    /// Ask the daemon which tool set this token opens (fails when the token is unknown or revoked).
    pub async fn describe(&self) -> Result<SessionDescription, ToolError> {
        let (status, bytes) = self
            .request("GET", SESSION_PATH, None)
            .await
            .map_err(ToolError::new)?;
        if status == 401 {
            return Err(ToolError::new(
                "the session token is unknown or was revoked",
            ));
        }
        serde_json::from_slice(&bytes)
            .map_err(|e| ToolError::new(format!("ratchetd answered HTTP {status}: {e}")))
    }
}

#[async_trait]
impl ToolBackend for SocketBackend {
    async fn call(&self, tool: &str, arguments: Value) -> Result<Value, ToolError> {
        let body = serde_json::to_vec(&json!({"tool": tool, "arguments": arguments}))
            .map_err(|e| ToolError::new(e.to_string()))?;
        let (status, bytes) = self
            .request("POST", CALL_PATH, Some(body))
            .await
            .map_err(ToolError::new)?;
        let parsed: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
        if (200..300).contains(&status) {
            return Ok(parsed.get("result").cloned().unwrap_or(Value::Null));
        }
        let message = parsed
            .pointer("/message")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("ratchetd answered HTTP {status}"));
        Err(ToolError::new(if status == 401 {
            "this session has ended (its token was revoked)".to_owned()
        } else {
            message
        }))
    }
}
