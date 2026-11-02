//! `grok agent stdio`: Grok's long-lived mode, the Agent Client Protocol (JSON-RPC 2.0 over stdio).
//!
//! Handshake: `initialize` → `authenticate` → `session/new` (or `session/load` to resume) with the session's MCP servers. A turn
//! is `session/prompt`: `session/update` notifications stream while it runs and its response (`stopReason`) ends it;
//! `session/cancel` (a notification) stops it early. Read-only is the tool allowlist: the sandbox is refused in this mode.
//! See `docs/harness-matrix.md`.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ratchet_core::ports::{
    Access, AgentEvent, AgentEventType, AgentUsage, Clock, LiveSession, McpServer, PortError,
    PortResult, SessionSpec, SharedEventSink,
};
use serde_json::{Map, Value, json};

use crate::classify::{FailureKind, FailureOnce, classify_status, classify_text};
use crate::cli_agent::{as_record, build_env, num, pointer, str_of, truncate};
use crate::detect::HarnessKind;
use crate::grok::GrokAdapter;
use crate::harness::{CLOSE_GRACE, HANDSHAKE_TIMEOUT, Scratch};
use crate::live::{RpcClient, RpcError, RpcMessage, parse_rpc, rpc_reply};
use crate::models::resolve_model;
use crate::process::SpawnOptions;

/// Maps ACP `session/update` notifications and the `session/prompt` response to `AgentEvent`s. Assistant text is reported as
/// `Delta` events while it streams and once more as a `Message` when the turn ends.
pub struct GrokAcpParser {
    pub session_id: Option<String>,
    pub malformed_lines: usize,
    pub unknown_types: BTreeSet<String>,
    text: String,
    started: BTreeSet<String>,
    finished: BTreeSet<String>,
    failure: FailureOnce,
    now: Box<dyn Fn() -> i64 + Send + Sync>,
}

/// The stop reason of a finished `session/prompt`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Stop {
    EndTurn,
    Cancelled,
    /// `max_tokens`, `max_turn_requests`, `refusal`, ...
    Other(String),
}

impl GrokAcpParser {
    pub fn new(now: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        Self {
            session_id: None,
            malformed_lines: 0,
            unknown_types: BTreeSet::new(),
            text: String::new(),
            started: BTreeSet::new(),
            finished: BTreeSet::new(),
            failure: FailureOnce::default(),
            now: Box::new(now),
        }
    }

    pub fn failure(&self) -> Option<FailureKind> {
        self.failure.kind()
    }

    /// One raw line (fixtures and tests): updates go through [`GrokAcpParser::update`]; a `session/prompt` response ends the turn
    /// ([`GrokAcpParser::finish_turn`]); other responses only teach the session id.
    pub fn parse_line(&mut self, line: &str) -> Vec<AgentEvent> {
        match parse_rpc(line) {
            Some(RpcMessage::Notification { method, params }) => {
                self.notification(&method, &params)
            }
            Some(RpcMessage::Response { result, error, .. }) => {
                if let Some(id) = result
                    .as_ref()
                    .and_then(|r| r.get("sessionId"))
                    .and_then(Value::as_str)
                {
                    self.session_id = Some(id.to_string());
                }
                match (result, error) {
                    (_, Some(e)) => self.error(&e),
                    (Some(r), None) if r.get("stopReason").is_some() => self.finish_turn(&r).0,
                    _ => Vec::new(),
                }
            }
            Some(RpcMessage::Request { method, .. }) => {
                self.unknown_types.insert(format!("request:{method}"));
                Vec::new()
            }
            None => {
                self.malformed_lines += 1;
                Vec::new()
            }
        }
    }

    pub fn notification(&mut self, method: &str, params: &Value) -> Vec<AgentEvent> {
        let at = (self.now)();
        if method != "session/update" {
            self.unknown_types.insert(method.to_string());
            return Vec::new();
        }
        let Some(update) = params.get("update").and_then(as_record) else {
            return Vec::new();
        };
        match str_of(update, "sessionUpdate") {
            Some("agent_message_chunk") => match chunk_text(update) {
                Some(t) => {
                    self.text.push_str(&t);
                    vec![AgentEvent::new(at, AgentEventType::Delta).with_text(t)]
                }
                None => Vec::new(),
            },
            Some("agent_thought_chunk") => match chunk_text(update) {
                Some(t) => vec![AgentEvent::new(at, AgentEventType::Thinking).with_text(t)],
                None => Vec::new(),
            },
            Some("tool_call") => self.tool_call(update, at),
            Some("tool_call_update") => self.tool_update(update, at),
            Some(
                "user_message_chunk"
                | "plan"
                | "available_commands_update"
                | "current_mode_update"
                | "config_option_update",
            ) => Vec::new(),
            Some(other) => {
                self.unknown_types.insert(format!("update:{other}"));
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    fn tool_call(&mut self, u: &Map<String, Value>, at: i64) -> Vec<AgentEvent> {
        let id = str_of(u, "toolCallId").unwrap_or("");
        if !id.is_empty() && !self.started.insert(id.to_string()) {
            return Vec::new();
        }
        let name = str_of(u, "toolName")
            .or_else(|| str_of(u, "title"))
            .unwrap_or("tool");
        let input = u.get("rawInput").cloned().unwrap_or(Value::Null);
        let detail = input.as_object().and_then(|o| {
            ["command", "path", "file_path", "query", "pattern"]
                .iter()
                .find_map(|k| str_of(o, k))
        });
        let text = match detail {
            Some(d) => format!("{name}: {}", truncate(d, 200)),
            None => name.to_string(),
        };
        vec![
            AgentEvent::new(at, AgentEventType::Tool)
                .with_text(text)
                .with_data(json!({"phase": "call", "id": id, "name": name, "input": input})),
        ]
    }

    fn tool_update(&mut self, u: &Map<String, Value>, at: i64) -> Vec<AgentEvent> {
        let id = str_of(u, "toolCallId").unwrap_or("");
        let status = str_of(u, "status");
        if !matches!(status, Some("completed" | "failed")) || !self.finished.insert(id.to_string())
        {
            return Vec::new();
        }
        let text: String = u
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|c| c.pointer("/content/text").and_then(Value::as_str))
            .collect::<Vec<_>>()
            .join("\n");
        let exit = u
            .get("rawOutput")
            .and_then(|o| o.get("exit_code"))
            .and_then(Value::as_i64);
        let is_error = status == Some("failed")
            || exit.is_some_and(|c| c != 0)
            || pointer(u, "/rawOutput/isError").and_then(Value::as_bool) == Some(true);
        vec![
            AgentEvent::new(at, AgentEventType::Tool)
                .with_text(truncate(&text, 300))
                .with_data(
                    json!({"phase": "result", "id": id, "exitCode": exit, "isError": is_error}),
                ),
        ]
    }

    /// The `session/prompt` response: the turn's `Message` (the chunks joined), its `Usage` when the result has one, and how it
    /// stopped.
    pub fn finish_turn(&mut self, result: &Value) -> (Vec<AgentEvent>, Stop) {
        let at = (self.now)();
        let mut events = Vec::new();
        let text = std::mem::take(&mut self.text);
        if !text.trim().is_empty() {
            events.push(AgentEvent::new(at, AgentEventType::Message).with_text(text));
        }
        let usage = result
            .get("usage")
            .or_else(|| result.pointer("/_meta/usage"))
            .and_then(as_record);
        if let Some(u) = usage {
            let n = |a: &str, b: &str| num(u.get(a)).max(num(u.get(b)));
            let usage = AgentUsage {
                input_tokens: n("inputTokens", "input_tokens"),
                output_tokens: n("outputTokens", "output_tokens"),
                cost_usd: result
                    .pointer("/_meta/total_cost_usd")
                    .or_else(|| u.get("total_cost_usd"))
                    .and_then(Value::as_f64)
                    .filter(|c| c.is_finite() && *c >= 0.0),
                ..AgentUsage::default()
            };
            events.push(AgentEvent::new(at, AgentEventType::Usage).with_usage(usage));
        }
        let empty = Map::new();
        let stop = match str_of(result.as_object().unwrap_or(&empty), "stopReason") {
            Some("end_turn") | None => Stop::EndTurn,
            Some("cancelled") => Stop::Cancelled,
            Some(other) => Stop::Other(other.to_string()),
        };
        if let Stop::Other(reason) = &stop {
            events.push(
                AgentEvent::new(at, AgentEventType::Error)
                    .with_text(format!("grok stopped before finishing: {reason}"))
                    .with_data(json!({"fatal": true, "stopReason": reason})),
            );
        }
        self.failure.reset();
        (events, stop)
    }

    /// A JSON-RPC `error` object: an Error event, classified by status (when the data carries one) or by its text.
    pub fn error(&mut self, error: &Value) -> Vec<AgentEvent> {
        let at = (self.now)();
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or("grok error")
            .to_string();
        let detail = error
            .pointer("/data/message")
            .and_then(Value::as_str)
            .unwrap_or("");
        let status = error
            .pointer("/data/status")
            .or_else(|| error.pointer("/data/httpStatus"))
            .and_then(Value::as_i64);
        let mut events = vec![
            AgentEvent::new(at, AgentEventType::Error)
                .with_text(message.clone())
                .with_data(json!({"fatal": true, "code": error.get("code")})),
        ];
        let kind = status
            .and_then(classify_status)
            .or_else(|| classify_text(&format!("{message} {detail}"), at / 1000));
        if let Some(kind) = kind {
            events.extend(self.failure.emit(at, kind, &message));
        }
        events
    }
}

/// `content` of a chunk is a block (`{type: "text", text}`).
fn chunk_text(update: &Map<String, Value>) -> Option<String> {
    pointer(update, "/content/text")
        .and_then(Value::as_str)
        .filter(|t| !t.is_empty())
        .map(str::to_string)
}

/// An ACP stdio MCP server: `env` is a list of `{name, value}`.
pub fn acp_mcp_servers(servers: &[McpServer]) -> Value {
    Value::Array(
        servers
            .iter()
            .map(|s| {
                json!({
                    "name": s.name,
                    "command": s.command,
                    "args": s.args,
                    "env": s.env.iter().map(|(k, v)| json!({"name": k, "value": v})).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

// ---------------------------------------------------------------- live session

/// `grok agent stdio`, held open for the whole chat.
pub struct GrokLiveSession {
    rpc: RpcClient,
    parser: Arc<Mutex<GrokAcpParser>>,
    session_id: String,
    turn: tokio::sync::Mutex<()>,
    busy: AtomicBool,
    events: SharedEventSink,
    clock: Arc<dyn Clock>,
    _scratch: Scratch,
}

impl GrokLiveSession {
    pub(crate) async fn open(
        adapter: &GrokAdapter,
        spec: &SessionSpec,
        events: SharedEventSink,
    ) -> PortResult<Box<dyn LiveSession>> {
        let scratch = Scratch::new("grok-live")?;
        let env = adapter.run_env(spec, None);
        let mut args = vec!["agent".to_string(), "stdio".to_string()];
        if spec.access == Access::ReadOnly {
            // the sandbox is refused in this mode: the tool allowlist is the read-only guarantee
            args.extend(adapter.read_only_tool_args());
        }
        args.extend(adapter.cli.extra_args.iter().cloned());
        let clock = adapter.cli.clock.clone();
        let parser = Arc::new(Mutex::new(GrokAcpParser::new({
            let clock = clock.clone();
            move || clock.now()
        })));
        let read_only = spec.access == Access::ReadOnly;
        let rpc = RpcClient::spawn(
            adapter.cli.spawner.as_ref(),
            &adapter.cli.binary,
            &args,
            &SpawnOptions {
                cwd: spec.workdir.clone().into(),
                env: build_env(&adapter.cli.base_env, Some(&env)),
            },
            true,
            {
                let parser = parser.clone();
                let events = events.clone();
                move |writer, msg| match msg {
                    RpcMessage::Notification { method, params } => {
                        let out = parser.lock().unwrap().notification(&method, &params);
                        for e in out {
                            events(e);
                        }
                    }
                    // nobody is there to answer a permission prompt: allow when editing is allowed, else refuse
                    RpcMessage::Request { id, method, params }
                        if method == "session/request_permission" =>
                    {
                        writer.write_json(&rpc_reply(
                            true,
                            &id,
                            permission_outcome(&params, read_only),
                        ));
                    }
                    RpcMessage::Request { id, method, .. } => {
                        parser
                            .lock()
                            .unwrap()
                            .unknown_types
                            .insert(format!("request:{method}"));
                        writer.write_json(&json!({
                            "jsonrpc": "2.0",
                            "id": id,
                            "error": {"code": -32601, "message": format!("{method} is not supported")},
                        }));
                    }
                    RpcMessage::Response { .. } => {}
                }
            },
            |_| {},
        );
        let fail = |what: &str, err: RpcError, rpc: &RpcClient| PortError::AgentFailed {
            message: format!("grok agent stdio {what}: {}", err.message()),
            exit_code: None,
            stderr: Some(rpc.process().stderr_tail()),
        };
        let init = rpc
            .request(
                "initialize",
                json!({"protocolVersion": 1, "clientCapabilities": {}}),
                HANDSHAKE_TIMEOUT,
            )
            .await
            .map_err(|e| fail("initialize", e, &rpc));
        let init = match init {
            Ok(v) => v,
            Err(e) => {
                rpc.process()
                    .close(std::time::Duration::from_millis(200))
                    .await;
                return Err(e);
            }
        };
        let has_key = ["XAI_API_KEY", "GROK_CODE_XAI_API_KEY"].iter().any(|k| {
            env.get(*k)
                .or_else(|| adapter.cli.base_env.get(*k))
                .is_some_and(|v| !v.is_empty())
        });
        let offered: Vec<&str> = init
            .get("authMethods")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|m| m.get("id").and_then(Value::as_str))
            .collect();
        let method = if has_key && (offered.is_empty() || offered.contains(&"xai.api_key")) {
            "xai.api_key"
        } else {
            "cached_token"
        };
        let mcp = acp_mcp_servers(&spec.mcp);
        let mut meta = Map::new();
        if let Some(text) = spec.system_append.as_ref().filter(|t| !t.is_empty()) {
            meta.insert("rules".into(), json!(text));
        }
        let handshake = async {
            rpc.request(
                "authenticate",
                json!({"methodId": method, "_meta": {"headless": true}}),
                HANDSHAKE_TIMEOUT,
            )
            .await
            .map_err(|e| ("authenticate", e))?;
            let mut params = json!({"cwd": spec.workdir, "mcpServers": mcp, "_meta": meta});
            let (rpc_method, resumed) = match spec.resume.as_ref().filter(|r| !r.is_empty()) {
                Some(id) => {
                    params["sessionId"] = json!(id);
                    ("session/load", Some(id.clone()))
                }
                None => ("session/new", None),
            };
            let result = rpc
                .request(rpc_method, params, HANDSHAKE_TIMEOUT)
                .await
                .map_err(|e| (rpc_method, e))?;
            let id = result
                .get("sessionId")
                .and_then(Value::as_str)
                .map(str::to_string)
                .or(resumed)
                .ok_or((
                    "session/new",
                    RpcError::Rpc(json!({"message": "no session id in the answer"})),
                ))?;
            if let Some(model) = spec.model.as_ref().filter(|m| !m.is_empty()) {
                let model = &resolve_model(HarnessKind::Grok, model).id;
                // best effort: a CLI that does not know the option keeps its default model
                let _ = rpc
                    .request(
                        "session/set_config_option",
                        json!({"sessionId": id, "configId": "model", "value": model}),
                        HANDSHAKE_TIMEOUT,
                    )
                    .await;
            }
            Ok::<_, (&str, RpcError)>(id)
        };
        let session_id = match handshake.await {
            Ok(id) => id,
            Err((what, err)) => {
                let e = fail(what, err.clone(), &rpc);
                // an auth failure here is worth reporting as one
                if let RpcError::Rpc(v) = &err {
                    for ev in parser.lock().unwrap().error(v) {
                        events(ev);
                    }
                }
                rpc.process()
                    .close(std::time::Duration::from_millis(200))
                    .await;
                return Err(e);
            }
        };
        parser.lock().unwrap().session_id = Some(session_id.clone());
        Ok(Box::new(Self {
            rpc,
            parser,
            session_id,
            turn: tokio::sync::Mutex::new(()),
            busy: AtomicBool::new(false),
            events,
            clock,
            _scratch: scratch,
        }))
    }
}

/// The answer to a `session/request_permission`: pick the option of the right kind.
fn permission_outcome(params: &Value, read_only: bool) -> Value {
    let want = if read_only { "reject" } else { "allow" };
    let option = params
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .find(|o| {
            o.get("kind")
                .and_then(Value::as_str)
                .is_some_and(|k| k.starts_with(want))
        })
        .and_then(|o| o.get("optionId"))
        .and_then(Value::as_str);
    match option {
        Some(id) => json!({"outcome": {"outcome": "selected", "optionId": id}}),
        None => json!({"outcome": {"outcome": "cancelled"}}),
    }
}

#[async_trait]
impl LiveSession for GrokLiveSession {
    async fn send(&self, text: &str) -> PortResult<()> {
        let _turn = self.turn.lock().await;
        self.busy.store(true, Ordering::SeqCst);
        let answer = self
            .rpc
            .request(
                "session/prompt",
                json!({"sessionId": self.session_id, "prompt": [{"type": "text", "text": text}]}),
                // a turn takes as long as it takes; `close` ends the process and wakes this with `Closed`
                std::time::Duration::from_secs(60 * 60 * 24),
            )
            .await;
        self.busy.store(false, Ordering::SeqCst);
        match answer {
            Ok(result) => {
                let (out, stop) = self.parser.lock().unwrap().finish_turn(&result);
                for e in out {
                    (self.events)(e);
                }
                match stop {
                    Stop::EndTurn | Stop::Cancelled => Ok(()),
                    Stop::Other(reason) => Err(PortError::agent_failed(format!(
                        "grok stopped before finishing: {reason}"
                    ))),
                }
            }
            Err(RpcError::Rpc(error)) => {
                let out = self.parser.lock().unwrap().error(&error);
                for e in out {
                    (self.events)(e);
                }
                let message = error
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("grok turn failed")
                    .to_string();
                Err(PortError::AgentFailed {
                    message,
                    exit_code: None,
                    stderr: Some(self.rpc.process().stderr_tail()),
                })
            }
            Err(other) => {
                let message = format!("grok agent stdio: {}", other.message());
                (self.events)(
                    AgentEvent::new(self.clock.now(), AgentEventType::Error)
                        .with_text(message.clone())
                        .with_data(json!({"stderr": self.rpc.process().stderr_tail()})),
                );
                Err(PortError::AgentFailed {
                    message,
                    exit_code: self.rpc.process().exit().and_then(|e| e.code),
                    stderr: Some(self.rpc.process().stderr_tail()),
                })
            }
        }
    }

    async fn interrupt(&self) -> PortResult<()> {
        if self.busy.load(Ordering::SeqCst) {
            // the pending `session/prompt` then answers with `stopReason: "cancelled"`
            self.rpc
                .notify("session/cancel", json!({"sessionId": self.session_id}));
        }
        Ok(())
    }

    fn session_id(&self) -> Option<String> {
        Some(self.session_id.clone())
    }

    async fn close(self: Box<Self>) {
        self.rpc.process().close(CLOSE_GRACE).await;
    }
}
