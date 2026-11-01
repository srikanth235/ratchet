//! Grok adapter: spawns `grok -p - --output-format streaming-json` and maps its JSONL to `AgentEvent`s.

use std::collections::{BTreeSet, HashSet};

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::cli_agent::{CliAgent, CliAgentOptions, as_record, num, str_of, truncate};
use crate::process::LineSplitter;
use ratchet_core::enums::AgentKind;
use ratchet_core::ports::{
    AgentAdapter, AgentEvent, AgentEventType, AgentSession, AgentSpec, AgentUsage,
    CancellationToken, EventSink, PortError, PortResult, Preflight,
};

#[derive(Debug, Clone, Default, PartialEq)]
pub struct GrokTurnResult {
    pub completed: bool,
    pub failed: bool,
    pub error: Option<String>,
    pub usage: Option<AgentUsage>,
}

/// Incremental parser for `grok --output-format streaming-json` JSONL. Same tolerance rules as the Claude and Codex parsers.
///
/// The stream is `thought` / `text` deltas, `tool_call` + `tool_call_update` pairs, a `usage` line per model step and one `end`
/// line that carries the session id, the stop reason and the totals (tokens and dollars). Text deltas are joined into one
/// `Message` that is flushed when something other than text arrives; thoughts and per-step usage are not reported.
pub struct GrokStreamParser {
    pub session_id: Option<String>,
    pub turn: GrokTurnResult,
    pub malformed_lines: usize,
    pub unknown_types: BTreeSet<String>,
    splitter: LineSplitter,
    text: String,
    started: HashSet<String>,
    finished: HashSet<String>,
    now: Box<dyn Fn() -> i64 + Send + Sync>,
}

impl GrokStreamParser {
    pub fn new(now: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        Self {
            session_id: None,
            turn: GrokTurnResult::default(),
            malformed_lines: 0,
            unknown_types: BTreeSet::new(),
            splitter: LineSplitter::new(),
            text: String::new(),
            started: HashSet::new(),
            finished: HashSet::new(),
            now: Box::new(now),
        }
    }

    pub fn push(&mut self, chunk: &str) -> Vec<AgentEvent> {
        let lines = self.splitter.push(chunk);
        lines.iter().flat_map(|l| self.parse_line(l)).collect()
    }

    /// Flush the partial last line and any text still buffered.
    pub fn end(&mut self) -> Vec<AgentEvent> {
        let lines = self.splitter.flush();
        let mut events: Vec<AgentEvent> = lines.iter().flat_map(|l| self.parse_line(l)).collect();
        events.extend(self.flush_text());
        events
    }

    fn flush_text(&mut self) -> Vec<AgentEvent> {
        let text = std::mem::take(&mut self.text);
        if text.trim().is_empty() {
            return Vec::new();
        }
        vec![AgentEvent::new((self.now)(), AgentEventType::Message).with_text(text)]
    }

    pub fn parse_line(&mut self, line: &str) -> Vec<AgentEvent> {
        let raw: Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => {
                self.malformed_lines += 1;
                return Vec::new();
            }
        };
        let Some(obj) = raw.as_object() else {
            self.malformed_lines += 1;
            return Vec::new();
        };
        let Some(kind) = str_of(obj, "type") else {
            self.malformed_lines += 1;
            return Vec::new();
        };
        let at = (self.now)();
        if kind == "text" {
            if let Some(delta) = str_of(obj, "data") {
                self.text.push_str(delta);
            }
            return Vec::new();
        }
        // Everything else ends the paragraph being streamed.
        let mut events = self.flush_text();
        match kind {
            // Reasoning, the slash-command list and the per-step usage carry nothing Ratchet reports.
            "thought" | "available_commands" | "usage" => {}
            "tool_call" => {
                let id = str_of(obj, "toolCallId").unwrap_or("");
                if id.is_empty() || self.started.insert(id.to_string()) {
                    let name = str_of(obj, "toolName")
                        .or_else(|| str_of(obj, "title"))
                        .unwrap_or("tool");
                    let input = obj.get("rawInput").cloned().unwrap_or(Value::Null);
                    let summary = tool_summary(name, &input);
                    events.push(
                        AgentEvent::new(at, AgentEventType::Tool)
                            .with_text(summary)
                            .with_data(
                                json!({"phase": "call", "id": id, "name": name, "input": input}),
                            ),
                    );
                }
            }
            "tool_call_update" => {
                let id = str_of(obj, "toolCallId").unwrap_or("");
                let status = str_of(obj, "status");
                if matches!(status, Some("completed" | "failed"))
                    && self.finished.insert(id.to_string())
                {
                    let raw_output = obj.get("rawOutput").and_then(as_record);
                    let exit_code = raw_output
                        .and_then(|o| o.get("exit_code"))
                        .and_then(Value::as_f64)
                        .map(|n| n as i64);
                    let is_error = status == Some("failed") || exit_code.is_some_and(|c| c != 0);
                    events.push(
                        AgentEvent::new(at, AgentEventType::Tool)
                            .with_text(truncate(&update_text(obj), 300))
                            .with_data(json!({
                                "phase": "result",
                                "id": id,
                                "exitCode": exit_code,
                                "isError": is_error,
                            })),
                    );
                }
            }
            "end" => events.extend(self.ended(obj, at)),
            "error" => {
                let message = str_of(obj, "message")
                    .or_else(|| str_of(obj, "data"))
                    .unwrap_or("grok error")
                    .to_string();
                self.turn = GrokTurnResult {
                    completed: false,
                    failed: true,
                    error: Some(message.clone()),
                    usage: None,
                };
                events.push(
                    AgentEvent::new(at, AgentEventType::Error)
                        .with_text(message)
                        .with_data(json!({"fatal": true})),
                );
            }
            other => {
                self.unknown_types.insert(other.to_string());
            }
        }
        events
    }

    fn ended(&mut self, raw: &serde_json::Map<String, Value>, at: i64) -> Vec<AgentEvent> {
        if let Some(id) = str_of(raw, "sessionId").filter(|id| !id.is_empty()) {
            self.session_id = Some(id.to_string());
        }
        let stop = str_of(raw, "stopReason").unwrap_or("end_turn");
        let empty = serde_json::Map::new();
        let u = raw.get("usage").and_then(as_record).unwrap_or(&empty);
        let cache_read = num(u.get("cache_read_input_tokens"));
        let cache_create = num(u.get("cache_creation_input_tokens"));
        let usage = AgentUsage {
            input_tokens: num(u.get("input_tokens")) + cache_read + cache_create,
            output_tokens: num(u.get("output_tokens")),
            cost_usd: raw
                .get("total_cost_usd")
                .and_then(Value::as_f64)
                .filter(|c| c.is_finite() && *c >= 0.0),
        };
        let mut events = vec![
            AgentEvent::new(at, AgentEventType::Usage)
                .with_usage(usage)
                .with_data(json!({
                    "cacheReadInputTokens": cache_read,
                    "cacheCreationInputTokens": cache_create,
                    "reasoningTokens": num(u.get("reasoning_tokens")),
                    "numTurns": num(raw.get("num_turns")),
                })),
        ];
        if stop == "end_turn" {
            self.turn = GrokTurnResult {
                completed: true,
                failed: false,
                error: None,
                usage: Some(usage),
            };
        } else {
            let message = format!("grok stopped before finishing: {stop}");
            self.turn = GrokTurnResult {
                completed: false,
                failed: true,
                error: Some(message.clone()),
                usage: Some(usage),
            };
            events.push(
                AgentEvent::new(at, AgentEventType::Error)
                    .with_text(message)
                    .with_data(json!({"fatal": true, "stopReason": stop})),
            );
        }
        events
    }
}

/// One line for the activity log: the shell command, the file, or the bare tool name.
fn tool_summary(name: &str, input: &Value) -> String {
    let field = |keys: &[&str]| {
        input
            .as_object()
            .and_then(|o| keys.iter().find_map(|k| str_of(o, k)))
    };
    match field(&[
        "command",
        "target_file",
        "file_path",
        "path",
        "query",
        "pattern",
    ]) {
        Some(detail) => format!("{name}: {}", truncate(detail, 200)),
        None => name.to_string(),
    }
}

/// The text a `tool_call_update` carries (the shell output, the file text, the error).
fn update_text(update: &serde_json::Map<String, Value>) -> String {
    update
        .get("content")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|c| {
            c.get("content")
                .and_then(|inner| inner.get("text"))
                .and_then(Value::as_str)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[derive(Clone, Default)]
pub struct GrokOptions {
    pub cli: CliAgentOptions,
    /// `--permission-mode`. Default `acceptEdits` (file edits and shell commands run without asking, as in a headless run).
    pub permission_mode: Option<String>,
    /// `--sandbox <PROFILE>` (filesystem and network access). Default: the CLI's own.
    pub sandbox: Option<String>,
    /// `--max-turns`. Default: the CLI's own.
    pub max_turns: Option<u32>,
    /// Set as `XAI_API_KEY` for the child (it takes precedence over a grok.com login).
    pub api_key: Option<String>,
    /// Skip the API-key preflight check: the CLI is signed in with a grok.com account (`grok login`) on this machine.
    pub allow_subscription_auth: bool,
}

pub struct GrokAdapter {
    cli: CliAgent,
    options: GrokOptions,
}

impl GrokAdapter {
    pub fn new(options: GrokOptions) -> Self {
        let mut cli = CliAgent::new("grok", options.cli.clone());
        if let Some(key) = &options.api_key {
            cli.base_env.insert("XAI_API_KEY".to_string(), key.clone());
        }
        Self { cli, options }
    }

    /// Argument list for one invocation, the prompt excluded (it goes on stdin through `-p -`). Exported for tests and docs.
    pub fn build_args(&self, spec: &AgentSpec, resume_id: Option<&str>) -> Vec<String> {
        let o = &self.options;
        let mut args: Vec<String> = ["-p", "-", "--output-format", "streaming-json"]
            .map(String::from)
            .to_vec();
        args.extend([
            "--permission-mode".to_string(),
            o.permission_mode
                .clone()
                .unwrap_or_else(|| "acceptEdits".to_string()),
        ]);
        if let Some(id) = resume_id.filter(|id| !id.is_empty()) {
            args.extend(["--resume".to_string(), id.to_string()]);
        }
        if let Some(model) = spec.model.as_ref().filter(|m| !m.is_empty()) {
            args.extend(["--model".to_string(), model.clone()]);
        }
        if let Some(sandbox) = o.sandbox.as_ref().filter(|s| !s.is_empty()) {
            args.extend(["--sandbox".to_string(), sandbox.clone()]);
        }
        if let Some(turns) = o.max_turns {
            args.extend(["--max-turns".to_string(), turns.to_string()]);
        }
        args.extend(self.cli.extra_args.iter().cloned());
        args
    }

    #[allow(clippy::too_many_arguments)]
    async fn turn(
        &self,
        session: &mut AgentSession,
        args: Vec<String>,
        spec: &AgentSpec,
        prompt: &str,
        on_event: &EventSink<'_>,
        cancel: &CancellationToken,
        is_start: bool,
    ) -> PortResult<()> {
        let clock = self.cli.clock.clone();
        let mut parser = GrokStreamParser::new(move || clock.now());
        let res = {
            let mut on_line = |line: &str| {
                for e in parser.parse_line(line) {
                    on_event(e);
                }
            };
            self.cli
                .run(&args, spec, Some(prompt.to_string()), &mut on_line, cancel)
                .await
        };
        for e in parser.end() {
            on_event(e);
        }
        if let Some(id) = parser.session_id.as_ref().filter(|id| !id.is_empty()) {
            session.id = id.clone();
        }

        let fail = |message: String| -> PortError {
            on_event(
                AgentEvent::new(self.cli.clock.now(), AgentEventType::Error)
                    .with_text(message.clone())
                    .with_data(json!({"exitCode": res.code(), "stderr": res.stderr_tail})),
            );
            PortError::AgentFailed {
                message,
                exit_code: res.code(),
                stderr: Some(res.stderr_tail.clone()),
            }
        };

        if res.aborted {
            on_event(
                AgentEvent::new(self.cli.clock.now(), AgentEventType::Status)
                    .with_text("grok turn aborted"),
            );
            return Err(PortError::Cancelled);
        }
        if let Some(err) = res.spawn_error() {
            return Err(fail(format!("cannot run '{}': {err}", self.cli.binary)));
        }
        if parser.turn.failed {
            return Err(PortError::AgentFailed {
                message: parser
                    .turn
                    .error
                    .clone()
                    .unwrap_or_else(|| "grok turn failed".to_string()),
                exit_code: res.code(),
                stderr: Some(res.stderr_tail.clone()),
            });
        }
        if !parser.turn.completed {
            let message = if res.code() != Some(0) {
                let how = res.signal().map(str::to_string).unwrap_or_else(|| {
                    format!(
                        "code {}",
                        res.code().map_or("null".into(), |c| c.to_string())
                    )
                });
                let tail = if res.stderr_tail.is_empty() {
                    String::new()
                } else {
                    format!(": {}", res.stderr_tail)
                };
                format!("grok exited with {how}{tail}")
            } else {
                "grok stream ended without an end event".to_string()
            };
            return Err(fail(message));
        }
        if is_start && session.id.is_empty() {
            return Err(fail("grok did not report a session id".to_string()));
        }
        Ok(())
    }
}

#[async_trait]
impl AgentAdapter for GrokAdapter {
    fn kind(&self) -> AgentKind {
        AgentKind::Grok
    }

    async fn preflight(&self) -> PortResult<Preflight> {
        let mut problems = Vec::new();
        if let Some(p) = self.cli.probe_binary().await {
            problems.push(p);
        }
        let has = |k: &str| self.cli.base_env.get(k).is_some_and(|v| !v.is_empty());
        if !self.options.allow_subscription_auth && !has("XAI_API_KEY") {
            problems.push("XAI_API_KEY is not set".to_string());
        }
        Ok(Preflight {
            ok: problems.is_empty(),
            problems,
        })
    }

    async fn start(
        &self,
        spec: &AgentSpec,
        on_event: &EventSink<'_>,
        cancel: Option<&CancellationToken>,
    ) -> PortResult<AgentSession> {
        let cancel = cancel.cloned().unwrap_or_default();
        let mut session = AgentSession {
            id: String::new(),
            kind: AgentKind::Grok,
        };
        self.turn(
            &mut session,
            self.build_args(spec, None),
            spec,
            &spec.prompt,
            on_event,
            &cancel,
            true,
        )
        .await?;
        Ok(session)
    }

    async fn resume(
        &self,
        session: &mut AgentSession,
        spec: &AgentSpec,
        message: &str,
        on_event: &EventSink<'_>,
        cancel: Option<&CancellationToken>,
    ) -> PortResult<()> {
        let cancel = cancel.cloned().unwrap_or_default();
        let args = self.build_args(spec, Some(&session.id));
        self.turn(session, args, spec, message, on_event, &cancel, false)
            .await
    }
}
