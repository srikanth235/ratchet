//! The stdio server: `initialize`, `ping`, `tools/list`, `tools/call`. Everything else is "method not found".

use std::sync::Arc;

use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};

use crate::protocol::{
    INVALID_PARAMS, INVALID_REQUEST, METHOD_NOT_FOUND, NOT_INITIALIZED, PARSE_ERROR, RpcError,
    error_message, negotiate, result_message,
};
use crate::tool::ToolProvider;

/// What `initialize` reports as `serverInfo`.
#[derive(Debug, Clone)]
pub struct ServerInfo {
    pub name: String,
    pub version: String,
    /// Shown to the model as the server's usage notes.
    pub instructions: Option<String>,
}

impl ServerInfo {
    pub fn ratchet(instructions: Option<&str>) -> Self {
        Self {
            name: "ratchet".into(),
            version: env!("CARGO_PKG_VERSION").into(),
            instructions: instructions.map(str::to_owned),
        }
    }
}

pub struct McpServer {
    info: ServerInfo,
    provider: Arc<dyn ToolProvider>,
}

impl McpServer {
    pub fn new(info: ServerInfo, provider: Arc<dyn ToolProvider>) -> Self {
        Self { info, provider }
    }

    /// Serve until the reader ends. Requests are answered in order.
    pub async fn serve<R, W>(&self, reader: R, mut writer: W) -> std::io::Result<()>
    where
        R: AsyncRead + Unpin,
        W: AsyncWrite + Unpin,
    {
        let mut lines = BufReader::new(reader).lines();
        let mut initialized = false;
        while let Some(line) = lines.next_line().await? {
            if line.trim().is_empty() {
                continue;
            }
            if let Some(reply) = self.handle_line(&line, &mut initialized).await {
                let mut text = reply.to_string();
                text.push('\n');
                writer.write_all(text.as_bytes()).await?;
                writer.flush().await?;
            }
        }
        Ok(())
    }

    /// One incoming line. `None` for notifications, which get no answer.
    pub async fn handle_line(&self, line: &str, initialized: &mut bool) -> Option<Value> {
        let msg: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(e) => {
                return Some(error_message(
                    &Value::Null,
                    &RpcError::new(PARSE_ERROR, format!("not JSON: {e}")),
                ));
            }
        };
        let Some(obj) = msg.as_object() else {
            return Some(error_message(
                &Value::Null,
                &RpcError::new(INVALID_REQUEST, "a message must be a JSON object"),
            ));
        };
        let id = obj.get("id").cloned();
        let Some(method) = obj.get("method").and_then(Value::as_str) else {
            // a response to a request we never sent, or junk
            return id.map(|id| {
                error_message(&id, &RpcError::new(INVALID_REQUEST, "missing \"method\""))
            });
        };
        let params = obj.get("params").cloned().unwrap_or(Value::Null);
        let Some(id) = id else {
            if method == "notifications/initialized" {
                *initialized = true;
            }
            return None;
        };
        let out = match method {
            "initialize" => {
                *initialized = true;
                Ok(self.initialize(&params))
            }
            "ping" => Ok(json!({})),
            _ if !*initialized => Err(RpcError::new(
                NOT_INITIALIZED,
                "send \"initialize\" before other requests",
            )),
            "tools/list" => Ok(json!({
                "tools": self.provider.tools().iter().map(crate::tool::Tool::to_json).collect::<Vec<_>>()
            })),
            "tools/call" => self.call(&params).await,
            other => Err(RpcError::new(
                METHOD_NOT_FOUND,
                format!("unknown method \"{other}\""),
            )),
        };
        Some(match out {
            Ok(result) => result_message(&id, result),
            Err(e) => error_message(&id, &e),
        })
    }

    fn initialize(&self, params: &Value) -> Value {
        let version = negotiate(params.get("protocolVersion").and_then(Value::as_str));
        let mut r = json!({
            "protocolVersion": version,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": self.info.name, "version": self.info.version},
        });
        if let Some(i) = &self.info.instructions {
            r["instructions"] = json!(i);
        }
        r
    }

    async fn call(&self, params: &Value) -> Result<Value, RpcError> {
        let name = params
            .get("name")
            .and_then(Value::as_str)
            .ok_or_else(|| RpcError::new(INVALID_PARAMS, "tools/call needs a \"name\""))?;
        let arguments = match params.get("arguments") {
            None | Some(Value::Null) => json!({}),
            Some(v) if v.is_object() => v.clone(),
            Some(_) => {
                return Err(RpcError::new(
                    INVALID_PARAMS,
                    "\"arguments\" must be an object",
                ));
            }
        };
        match self.provider.call(name, arguments).await {
            Ok(out) => Ok(out.to_json()),
            Err(refusal) => Err(RpcError::new(INVALID_PARAMS, refusal.0)),
        }
    }
}
