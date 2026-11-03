//! What a tool is, and what answers for a set of them.

use async_trait::async_trait;
use serde_json::{Value, json};

/// Checks `arguments` against the tool's typed arguments before the call is forwarded.
pub type Validator = fn(&Value) -> Result<(), String>;

/// One tool as `tools/list` shows it.
#[derive(Debug, Clone)]
pub struct Tool {
    pub name: String,
    pub description: String,
    /// JSON Schema of the arguments (an object schema).
    pub input_schema: Value,
    /// Changes nothing. Clients may skip the approval prompt for these.
    pub read_only: bool,
    /// Deletes, spends or sends. Only meaningful when `!read_only`.
    pub destructive: bool,
    /// Reaches outside this machine (GitHub, a model API).
    pub open_world: bool,
    pub validate: Option<Validator>,
}

impl Tool {
    pub fn to_json(&self) -> Value {
        let mut annotations = json!({
            "readOnlyHint": self.read_only,
            "openWorldHint": self.open_world,
        });
        if !self.read_only {
            annotations["destructiveHint"] = json!(self.destructive);
        }
        json!({
            "name": self.name,
            "description": self.description,
            "inputSchema": self.input_schema,
            "annotations": annotations,
        })
    }
}

/// A finished call. `is_error` means the tool ran and failed; the model reads `text` and can try again.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub text: String,
    pub structured: Option<Value>,
    pub is_error: bool,
}

impl ToolOutput {
    /// A JSON value as the result: pretty text for the model, and `structuredContent` when it is an object.
    pub fn json(value: Value) -> Self {
        let text = serde_json::to_string_pretty(&value).unwrap_or_default();
        let structured = value.is_object().then_some(value);
        Self {
            text,
            structured,
            is_error: false,
        }
    }

    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            structured: None,
            is_error: false,
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            text: message.into(),
            structured: None,
            is_error: true,
        }
    }

    pub fn to_json(&self) -> Value {
        let mut v = json!({
            "content": [{"type": "text", "text": self.text}],
            "isError": self.is_error,
        });
        if let Some(s) = &self.structured {
            v["structuredContent"] = s.clone();
        }
        v
    }
}

/// A call the server refuses outright (a JSON-RPC error, not a tool result): the tool is not in this session's set.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct Refusal(pub String);

/// Why a backend call failed; the model sees the message.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct ToolError(pub String);

impl ToolError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

/// What the stdio server asks: which tools exist and how to run one.
#[async_trait]
pub trait ToolProvider: Send + Sync {
    fn tools(&self) -> Vec<Tool>;
    async fn call(&self, name: &str, arguments: Value) -> Result<ToolOutput, Refusal>;
}

/// Runs the tools of a session set against something real (the daemon over its socket, the runner's inbox, a test double).
#[async_trait]
pub trait ToolBackend: Send + Sync {
    async fn call(&self, tool: &str, arguments: Value) -> Result<Value, ToolError>;
}
