//! `codex app-server`: Codex's long-lived mode, JSONL JSON-RPC over stdio (the `"jsonrpc"` key is left out).
//!
//! Handshake: `initialize` → `initialized` notification → `thread/start` (or `thread/resume`). A turn is `turn/start` (its response
//! carries the turn id) followed by notifications until `turn/completed`; `turn/interrupt {threadId, turnId}` ends it early.
//! The parser maps the notifications to `AgentEvent`s; [`CodexLiveSession`] drives the process. See `docs/harness-matrix.md`.

use std::collections::BTreeSet;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ratchet_core::ports::{
    Access, AgentEvent, AgentEventType, AgentUsage, Clock, LiveSession, PortError, PortResult,
    SessionSpec, SharedEventSink,
};
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

use crate::classify::{FailureKind, FailureOnce, classify_status, classify_text};
use crate::cli_agent::{as_record, build_env, num, pointer, str_of, truncate};
use crate::codex::{CodexAdapter, CodexPricing};
use crate::detect::HarnessKind;
use crate::harness::{CLOSE_GRACE, HANDSHAKE_TIMEOUT, Scratch};
use crate::live::{RpcClient, RpcError, RpcMessage, parse_rpc};
use crate::models::resolve_model;
use crate::process::{ProcessExit, SpawnOptions};

/// How a turn ended, as `turn/completed` says.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TurnSignal {
    Completed,
    Failed(String),
    Interrupted,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct Tokens {
    input: u64,
    cached: u64,
    output: u64,
}

fn tokens_of(v: Option<&Value>) -> Option<Tokens> {
    let o = v.and_then(as_record)?;
    Some(Tokens {
        input: num(o.get("inputTokens")),
        cached: num(o.get("cachedInputTokens")),
        output: num(o.get("outputTokens")),
    })
}

/// Maps `codex app-server` messages to `AgentEvent`s. Same tolerance rules as the exec parser: anything unknown is counted.
pub struct CodexAppServerParser {
    pub thread_id: Option<String>,
    pub turn_id: Option<String>,
    pub malformed_lines: usize,
    pub unknown_types: BTreeSet<String>,
    /// The signal of the last `turn/completed` line seen by [`CodexAppServerParser::parse_line`] (tests; the live session reads
    /// the one `handle` returns).
    pub last_signal: Option<TurnSignal>,
    seen_total: Tokens,
    turn_tokens: Option<Tokens>,
    reset_hint: Option<i64>,
    last_error: Option<String>,
    failure: FailureOnce,
    pricing: Option<CodexPricing>,
    now: Box<dyn Fn() -> i64 + Send + Sync>,
}

impl CodexAppServerParser {
    pub fn new(
        now: impl Fn() -> i64 + Send + Sync + 'static,
        pricing: Option<CodexPricing>,
    ) -> Self {
        Self {
            thread_id: None,
            turn_id: None,
            malformed_lines: 0,
            unknown_types: BTreeSet::new(),
            last_signal: None,
            seen_total: Tokens::default(),
            turn_tokens: None,
            reset_hint: None,
            last_error: None,
            failure: FailureOnce::default(),
            pricing,
            now: Box::new(now),
        }
    }

    /// How the last turn failed, when it was a rate limit or an expired login.
    pub fn failure(&self) -> Option<FailureKind> {
        self.failure.kind()
    }

    /// One raw line (for fixtures and tests). Responses only teach the parser the thread and turn ids.
    pub fn parse_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let Some(msg) = parse_rpc(line) else {
            self.malformed_lines += 1;
            return Vec::new();
        };
        self.response_ids(&msg);
        let (events, signal) = self.handle(&msg);
        if signal.is_some() {
            self.last_signal = signal;
        }
        events
    }

    /// The thread / turn ids a response carries (`thread/start`, `thread/resume`, `turn/start`).
    pub fn response_ids(&mut self, msg: &RpcMessage) {
        if let RpcMessage::Response {
            result: Some(r), ..
        } = msg
        {
            self.learn_ids(r);
        }
    }

    pub fn learn_ids(&mut self, result: &Value) {
        if let Some(id) = result
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .filter(|i| !i.is_empty())
        {
            self.thread_id = Some(id.to_string());
        }
        if let Some(id) = result
            .pointer("/turn/id")
            .and_then(Value::as_str)
            .filter(|i| !i.is_empty())
        {
            self.turn_id = Some(id.to_string());
        }
    }

    /// Events (and, at `turn/completed`, the turn's outcome) for one message from the server.
    pub fn handle(&mut self, msg: &RpcMessage) -> (Vec<AgentEvent>, Option<TurnSignal>) {
        let at = (self.now)();
        match msg {
            RpcMessage::Notification { method, params } => self.notification(method, params, at),
            RpcMessage::Request { method, .. } => {
                // approvals are never asked for (`approvalPolicy: "never"`); count anything else the server wants from us
                self.unknown_types.insert(format!("request:{method}"));
                (Vec::new(), None)
            }
            RpcMessage::Response { .. } => (Vec::new(), None),
        }
    }

    fn notification(
        &mut self,
        method: &str,
        params: &Value,
        at: i64,
    ) -> (Vec<AgentEvent>, Option<TurnSignal>) {
        let empty = Map::new();
        let p = params.as_object().unwrap_or(&empty);
        let mut events = Vec::new();
        match method {
            "thread/started" => {
                self.learn_ids(params);
                events.push(
                    AgentEvent::new(at, AgentEventType::Status)
                        .with_text("codex thread started")
                        .with_data(json!({"sessionId": self.thread_id})),
                );
            }
            "turn/started" => {
                self.failure.reset();
                self.last_error = None;
                self.learn_ids(params);
            }
            "item/agentMessage/delta" => {
                if let Some(d) = str_of(p, "delta").filter(|d| !d.is_empty()) {
                    events.push(AgentEvent::new(at, AgentEventType::Delta).with_text(d));
                }
            }
            "item/reasoning/textDelta" | "item/reasoning/summaryTextDelta" => {
                if let Some(d) = str_of(p, "delta").filter(|d| !d.is_empty()) {
                    events.push(AgentEvent::new(at, AgentEventType::Thinking).with_text(d));
                }
            }
            "item/started" => {
                if let Some(item) = p.get("item").and_then(as_record) {
                    events.extend(self.item(item, at, true));
                }
            }
            "item/completed" => {
                if let Some(item) = p.get("item").and_then(as_record) {
                    events.extend(self.item(item, at, false));
                }
            }
            "thread/tokenUsage/updated" => self.token_usage(p),
            "account/rateLimits/updated" => self.rate_limits(p),
            "error" => {
                let err = p.get("error").and_then(as_record).unwrap_or(&empty);
                let message = str_of(err, "message").unwrap_or("codex error").to_string();
                let will_retry = p.get("willRetry").and_then(Value::as_bool) == Some(true);
                self.last_error = Some(message.clone());
                events.push(
                    AgentEvent::new(at, AgentEventType::Error)
                        .with_text(message.clone())
                        .with_data(json!({"fatal": false})),
                );
                if !will_retry {
                    events.extend(self.classify_error(at, err.get("codexErrorInfo"), &message));
                }
            }
            "turn/completed" => return self.turn_completed(p, at, events),
            // bookkeeping the chat does not show
            "thread/status/changed"
            | "thread/name/updated"
            | "turn/diff/updated"
            | "turn/plan/updated"
            | "item/commandExecution/outputDelta"
            | "item/fileChange/outputDelta"
            | "item/mcpToolCall/progress"
            | "mcpServer/startupStatus/updated"
            | "configWarning"
            | "deprecationNotice" => {}
            other => {
                self.unknown_types.insert(other.to_string());
            }
        }
        (events, None)
    }

    fn item(&mut self, item: &Map<String, Value>, at: i64, started: bool) -> Vec<AgentEvent> {
        let id = str_of(item, "id").unwrap_or("");
        match str_of(item, "type") {
            Some("agentMessage") if !started => match str_of(item, "text") {
                Some(t) if !t.trim().is_empty() => {
                    vec![AgentEvent::new(at, AgentEventType::Message).with_text(t)]
                }
                _ => Vec::new(),
            },
            Some("commandExecution") => {
                if started {
                    let command = str_of(item, "command").unwrap_or("");
                    vec![
                        AgentEvent::new(at, AgentEventType::Tool)
                            .with_text(format!("command: {}", truncate(command, 200)))
                            .with_data(json!({"phase": "call", "id": id, "name": "command", "input": {"command": command}})),
                    ]
                } else {
                    let exit = item.get("exitCode").and_then(Value::as_i64);
                    let failed =
                        str_of(item, "status") == Some("failed") || exit.is_some_and(|c| c != 0);
                    vec![
                        AgentEvent::new(at, AgentEventType::Tool)
                            .with_text(truncate(str_of(item, "aggregatedOutput").unwrap_or(""), 300))
                            .with_data(json!({"phase": "result", "id": id, "exitCode": exit, "isError": failed})),
                    ]
                }
            }
            Some("mcpToolCall") => {
                let name = format!(
                    "{}.{}",
                    str_of(item, "server").unwrap_or("mcp"),
                    str_of(item, "tool").unwrap_or("")
                );
                if started {
                    vec![
                        AgentEvent::new(at, AgentEventType::Tool)
                            .with_text(name.clone())
                            .with_data(json!({"phase": "call", "id": id, "name": name, "input": item.get("arguments").cloned().unwrap_or(Value::Null)})),
                    ]
                } else {
                    let failed = str_of(item, "status") == Some("failed")
                        || item.get("error").is_some_and(|e| !e.is_null());
                    let text = pointer(item, "/result/content/0/text")
                        .and_then(Value::as_str)
                        .or_else(|| pointer(item, "/error/message").and_then(Value::as_str))
                        .unwrap_or("");
                    vec![
                        AgentEvent::new(at, AgentEventType::Tool)
                            .with_text(truncate(text, 300))
                            .with_data(json!({"phase": "result", "id": id, "isError": failed})),
                    ]
                }
            }
            Some("fileChange") if !started => {
                let changes = item.get("changes").cloned().unwrap_or(Value::Null);
                let paths: Vec<&str> = changes
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c.get("path").and_then(Value::as_str))
                    .collect();
                let failed = str_of(item, "status") == Some("failed");
                vec![
                    AgentEvent::new(at, AgentEventType::Tool)
                        .with_text(format!("apply_patch: {}", paths.join(", ")))
                        .with_data(json!({"phase": "call", "id": id, "name": "apply_patch", "input": {"changes": changes}, "isError": failed})),
                ]
            }
            Some("webSearch") if !started => vec![
                AgentEvent::new(at, AgentEventType::Tool)
                    .with_text("web_search")
                    .with_data(json!({"phase": "call", "id": id, "name": "web_search"})),
            ],
            // the user's own message echoed back, reasoning (its text arrives as deltas), and the other bookkeeping items
            Some("userMessage" | "reasoning" | "agentMessage" | "fileChange" | "webSearch") => {
                Vec::new()
            }
            Some(other) => {
                self.unknown_types.insert(format!("item:{other}"));
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    /// `tokenUsage.total` is the thread's running total; the turn's usage is what it grew by.
    fn token_usage(&mut self, p: &Map<String, Value>) {
        let Some(total) = tokens_of(pointer(p, "/tokenUsage/total")) else {
            return;
        };
        let seen = self.seen_total;
        let delta = if total.input < seen.input || total.output < seen.output {
            total // the counter restarted
        } else {
            Tokens {
                input: total.input - seen.input,
                cached: total.cached.saturating_sub(seen.cached),
                output: total.output - seen.output,
            }
        };
        self.seen_total = total;
        let acc = self.turn_tokens.get_or_insert_with(Tokens::default);
        acc.input += delta.input;
        acc.cached += delta.cached;
        acc.output += delta.output;
    }

    /// The reset time of the quota window that is full, as the server last reported it.
    fn rate_limits(&mut self, p: &Map<String, Value>) {
        let Some(limits) = p.get("rateLimits").and_then(as_record) else {
            return;
        };
        let full = ["primary", "secondary"]
            .iter()
            .filter_map(|k| limits.get(*k).and_then(as_record))
            .filter(|w| w.get("usedPercent").and_then(Value::as_f64).unwrap_or(0.0) >= 100.0)
            .filter_map(|w| w.get("resetsAt").and_then(Value::as_i64))
            .max();
        if full.is_some() {
            self.reset_hint = full;
        }
    }

    fn classify_error(
        &mut self,
        at: i64,
        info: Option<&Value>,
        message: &str,
    ) -> Option<AgentEvent> {
        // `codexErrorInfo` is a string for unit variants and `{Variant: {...}}` for the ones with fields
        let (name, status) = match info {
            Some(Value::String(s)) => (s.to_ascii_lowercase(), None),
            Some(Value::Object(o)) => o.iter().next().map_or((String::new(), None), |(k, v)| {
                (
                    k.to_ascii_lowercase(),
                    v.get("httpStatusCode").and_then(Value::as_i64),
                )
            }),
            _ => (String::new(), None),
        };
        let kind = if name.contains("usagelimit") || name.contains("ratelimit") {
            Some(FailureKind::RateLimited {
                resets_at: self.reset_hint,
            })
        } else if name.contains("unauthorized") {
            Some(FailureKind::AuthExpired)
        } else {
            status
                .and_then(classify_status)
                .or_else(|| classify_text(message, at / 1000))
        };
        let kind = match kind {
            Some(FailureKind::RateLimited { resets_at: None }) => FailureKind::RateLimited {
                resets_at: self.reset_hint,
            },
            Some(k) => k,
            None => return None,
        };
        self.failure.emit(at, kind, message)
    }

    fn turn_completed(
        &mut self,
        p: &Map<String, Value>,
        at: i64,
        mut events: Vec<AgentEvent>,
    ) -> (Vec<AgentEvent>, Option<TurnSignal>) {
        let turn = p.get("turn").and_then(as_record);
        let status = turn
            .and_then(|t| str_of(t, "status"))
            .unwrap_or("completed");
        if let Some(tokens) = self.turn_tokens.take() {
            let mut usage = AgentUsage {
                input_tokens: tokens.input,
                output_tokens: tokens.output,
                cost_usd: None,
            };
            if let Some(price) = &self.pricing {
                let uncached = tokens.input.saturating_sub(tokens.cached) as f64;
                usage.cost_usd = Some(
                    (uncached * price.input_per_m
                        + tokens.cached as f64
                            * price.cached_input_per_m.unwrap_or(price.input_per_m)
                        + tokens.output as f64 * price.output_per_m)
                        / 1e6,
                );
            }
            events.push(
                AgentEvent::new(at, AgentEventType::Usage)
                    .with_usage(usage)
                    .with_data(json!({"cachedInputTokens": tokens.cached})),
            );
        }
        let signal = match status {
            "failed" => {
                let err = turn.and_then(|t| t.get("error")).and_then(as_record);
                let message = err
                    .and_then(|e| str_of(e, "message"))
                    .map(str::to_string)
                    .or_else(|| self.last_error.clone())
                    .unwrap_or_else(|| "codex turn failed".to_string());
                if self.last_error.is_none() {
                    events.push(
                        AgentEvent::new(at, AgentEventType::Error)
                            .with_text(message.clone())
                            .with_data(json!({"fatal": true})),
                    );
                    events.extend(self.classify_error(
                        at,
                        err.and_then(|e| e.get("codexErrorInfo")),
                        &message,
                    ));
                }
                TurnSignal::Failed(message)
            }
            "interrupted" => {
                events.push(
                    AgentEvent::new(at, AgentEventType::Status).with_text("codex turn interrupted"),
                );
                TurnSignal::Interrupted
            }
            _ => TurnSignal::Completed,
        };
        (events, Some(signal))
    }
}

// ---------------------------------------------------------------- live session

enum Note {
    Turn(TurnSignal),
    Exited(String),
}

/// `codex app-server`, held open for the whole chat.
pub struct CodexLiveSession {
    rpc: RpcClient,
    notes: tokio::sync::Mutex<mpsc::UnboundedReceiver<Note>>,
    parser: Arc<Mutex<CodexAppServerParser>>,
    thread_id: String,
    busy: AtomicBool,
    events: SharedEventSink,
    clock: Arc<dyn Clock>,
    _scratch: Scratch,
}

impl CodexLiveSession {
    pub(crate) async fn open(
        adapter: &CodexAdapter,
        spec: &SessionSpec,
        events: SharedEventSink,
    ) -> PortResult<Box<dyn LiveSession>> {
        let scratch = Scratch::new("codex-live")?;
        let overlay = adapter.write_overlay(spec, &scratch)?;
        let args = adapter.build_app_server_args(spec);
        let env = adapter.run_env(spec, overlay.as_deref());
        let clock = adapter.cli.clock.clone();
        let parser = Arc::new(Mutex::new(CodexAppServerParser::new(
            {
                let clock = clock.clone();
                move || clock.now()
            },
            adapter.pricing_for(spec),
        )));
        let (tx, rx) = mpsc::unbounded_channel();
        let rpc = RpcClient::spawn(
            adapter.cli.spawner.as_ref(),
            &adapter.cli.binary,
            &args,
            &SpawnOptions {
                cwd: spec.workdir.clone().into(),
                env: build_env(&adapter.cli.base_env, Some(&env)),
            },
            false,
            {
                let parser = parser.clone();
                let events = events.clone();
                let tx = tx.clone();
                move |_, msg| {
                    let (out, signal) = parser.lock().unwrap().handle(&msg);
                    for e in out {
                        events(e);
                    }
                    if let Some(s) = signal {
                        let _ = tx.send(Note::Turn(s));
                    }
                }
            },
            {
                let tx = tx.clone();
                move |exit: &ProcessExit| {
                    let how = exit.spawn_error.clone().unwrap_or_else(|| {
                        exit.signal.clone().unwrap_or_else(|| {
                            format!(
                                "code {}",
                                exit.code.map_or("null".into(), |c| c.to_string())
                            )
                        })
                    });
                    let _ = tx.send(Note::Exited(how));
                }
            },
        );
        let handshake = async {
            rpc.request(
                "initialize",
                json!({"clientInfo": {"name": "ratchet", "title": "Ratchet", "version": env!("CARGO_PKG_VERSION")}}),
                HANDSHAKE_TIMEOUT,
            )
            .await?;
            rpc.notify("initialized", json!({}));
            let access = match spec.access {
                Access::ReadOnly => "read-only",
                Access::Edit { .. } => "workspace-write",
            };
            let mut params = json!({
                "cwd": spec.workdir,
                "approvalPolicy": "never",
                "sandbox": access,
            });
            if let Some(model) = spec.model.as_ref().filter(|m| !m.is_empty()) {
                let model = &resolve_model(HarnessKind::Codex, model).id;
                params["model"] = json!(model);
            }
            match spec.resume.as_ref().filter(|r| !r.is_empty()) {
                Some(id) => {
                    params["threadId"] = json!(id);
                    rpc.request("thread/resume", params, HANDSHAKE_TIMEOUT)
                        .await
                }
                None => rpc.request("thread/start", params, HANDSHAKE_TIMEOUT).await,
            }
        };
        let started = match handshake.await {
            Ok(v) => v,
            Err(err) => {
                let stderr = rpc.process().stderr_tail();
                rpc.process().close(Duration::from_millis(200)).await;
                return Err(PortError::AgentFailed {
                    message: format!("codex app-server did not start: {}", err.message()),
                    exit_code: None,
                    stderr: Some(stderr),
                });
            }
        };
        let thread_id = {
            let mut p = parser.lock().unwrap();
            p.learn_ids(&started);
            p.thread_id.clone()
        }
        .or_else(|| spec.resume.clone())
        .ok_or_else(|| PortError::agent_failed("codex app-server did not report a thread id"))?;
        Ok(Box::new(Self {
            rpc,
            notes: tokio::sync::Mutex::new(rx),
            parser,
            thread_id,
            busy: AtomicBool::new(false),
            events,
            clock,
            _scratch: scratch,
        }))
    }

    fn fail(&self, message: String) -> PortResult<()> {
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

#[async_trait]
impl LiveSession for CodexLiveSession {
    async fn send(&self, text: &str) -> PortResult<()> {
        let mut notes = self.notes.lock().await;
        while notes.try_recv().is_ok() {}
        self.busy.store(true, Ordering::SeqCst);
        let started = self
            .rpc
            .request(
                "turn/start",
                json!({"threadId": self.thread_id, "input": [{"type": "text", "text": text}]}),
                HANDSHAKE_TIMEOUT,
            )
            .await;
        let result = match started {
            Ok(r) => {
                self.parser.lock().unwrap().learn_ids(&r);
                match notes.recv().await {
                    Some(Note::Turn(TurnSignal::Completed | TurnSignal::Interrupted)) => Ok(()),
                    // the parser already reported the error and its classification
                    Some(Note::Turn(TurnSignal::Failed(message))) => Err(PortError::AgentFailed {
                        message,
                        exit_code: None,
                        stderr: Some(self.rpc.process().stderr_tail()),
                    }),
                    Some(Note::Exited(how)) => {
                        self.fail(format!("codex app-server exited with {how}"))
                    }
                    None => self.fail("codex app-server ended".to_string()),
                }
            }
            Err(RpcError::Rpc(e)) => {
                let message = e
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("turn/start failed")
                    .to_string();
                let mut p = self.parser.lock().unwrap();
                let at = self.clock.now();
                let mut out = vec![
                    AgentEvent::new(at, AgentEventType::Error)
                        .with_text(message.clone())
                        .with_data(json!({"fatal": true})),
                ];
                out.extend(p.classify_error(
                    at,
                    e.get("data").and_then(|d| d.get("codexErrorInfo")),
                    &message,
                ));
                drop(p);
                for ev in out {
                    (self.events)(ev);
                }
                Err(PortError::agent_failed(message))
            }
            Err(other) => self.fail(format!("codex app-server: {}", other.message())),
        };
        self.busy.store(false, Ordering::SeqCst);
        result
    }

    async fn interrupt(&self) -> PortResult<()> {
        if !self.busy.load(Ordering::SeqCst) {
            return Ok(());
        }
        let turn_id = self.parser.lock().unwrap().turn_id.clone();
        let Some(turn_id) = turn_id else {
            return Ok(());
        };
        // the turn ends with `turn/completed` (interrupted), which is what `send` waits for
        match self
            .rpc
            .request(
                "turn/interrupt",
                json!({"threadId": self.thread_id, "turnId": turn_id}),
                HANDSHAKE_TIMEOUT,
            )
            .await
        {
            Ok(_) | Err(RpcError::Closed) => Ok(()),
            Err(e) => Err(PortError::agent_failed(format!(
                "codex turn/interrupt failed: {}",
                e.message()
            ))),
        }
    }

    fn session_id(&self) -> Option<String> {
        Some(self.thread_id.clone())
    }

    async fn close(self: Box<Self>) {
        self.rpc.process().close(CLOSE_GRACE).await;
    }
}
