//! Claude Code adapter: spawns `claude -p --output-format stream-json --verbose` and maps its stream to `AgentEvent`s.

use std::collections::BTreeSet;

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::cli_agent::{CliAgent, CliAgentOptions, as_record, num, str_of, truncate};
use crate::domain::{
    AgentAdapter, AgentError, AgentEvent, AgentEventType, AgentKind, AgentSession, AgentSpec, AgentUsage, EventCallback, Preflight,
};
use crate::process::LineSplitter;

#[derive(Debug, Clone, PartialEq)]
pub struct ClaudeResult {
    pub subtype: String,
    pub is_error: bool,
    pub text: String,
    pub num_turns: u64,
    pub duration_ms: u64,
    pub usage: AgentUsage,
}

fn tool_summary(name: &str, input: Option<&Value>) -> String {
    if let Some(input) = input.and_then(as_record) {
        for key in ["command", "file_path", "path", "pattern", "url", "description"] {
            if let Some(v) = str_of(input, key) {
                return format!("{name}: {}", truncate(v, 200));
            }
        }
        let json = serde_json::to_string(input).unwrap_or_default();
        return format!("{name}: {}", truncate(&json, 200));
    }
    name.to_string()
}

fn block_text(content: Option<&Value>) -> String {
    match content {
        Some(Value::String(s)) => s.clone(),
        Some(Value::Array(blocks)) => blocks
            .iter()
            .map(|b| b.as_object().and_then(|o| str_of(o, "text")).unwrap_or(""))
            .filter(|t| !t.is_empty())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

fn set(map: &Map<String, Value>, key: &str) -> Value {
    map.get(key).cloned().unwrap_or(Value::Null)
}

/// Incremental parser for `claude -p --output-format stream-json --verbose` output.
///
/// Tolerant by design: partial lines are buffered, non-JSON lines and unknown event/block types are counted and skipped, never
/// raised. Usage is reported once per turn, from the `result` line.
pub struct ClaudeStreamParser {
    pub session_id: Option<String>,
    pub model: Option<String>,
    pub result: Option<ClaudeResult>,
    pub malformed_lines: usize,
    pub unknown_types: BTreeSet<String>,
    splitter: LineSplitter,
    now: Box<dyn Fn() -> i64 + Send + Sync>,
}

impl ClaudeStreamParser {
    pub fn new(now: impl Fn() -> i64 + Send + Sync + 'static) -> Self {
        Self {
            session_id: None,
            model: None,
            result: None,
            malformed_lines: 0,
            unknown_types: BTreeSet::new(),
            splitter: LineSplitter::new(),
            now: Box::new(now),
        }
    }

    pub fn push(&mut self, chunk: &str) -> Vec<AgentEvent> {
        let lines = self.splitter.push(chunk);
        lines.iter().flat_map(|l| self.parse_line(l)).collect()
    }

    pub fn end(&mut self) -> Vec<AgentEvent> {
        let lines = self.splitter.flush();
        lines.iter().flat_map(|l| self.parse_line(l)).collect()
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
        if let Some(id) = str_of(obj, "session_id") {
            if self.session_id.as_deref().is_none_or(str::is_empty) {
                self.session_id = Some(id.to_string());
            }
        }
        let at = (self.now)();
        match kind {
            "system" => self.system(obj, at),
            "assistant" => self.assistant(obj, at),
            "user" => self.user(obj, at),
            "result" => self.result_line(obj, at),
            other => {
                self.unknown_types.insert(other.to_string());
                Vec::new()
            }
        }
    }

    fn system(&mut self, raw: &Map<String, Value>, at: i64) -> Vec<AgentEvent> {
        let subtype = raw.get("subtype");
        if subtype.and_then(Value::as_str) == Some("init") {
            if let Some(id) = str_of(raw, "session_id") {
                self.session_id = Some(id.to_string());
            }
            if let Some(model) = str_of(raw, "model") {
                self.model = Some(model.to_string());
            }
            let model_note = self.model.as_deref().filter(|m| !m.is_empty()).map(|m| format!(" ({m})")).unwrap_or_default();
            return vec![
                AgentEvent::new(at, AgentEventType::Status)
                    .with_text(format!("claude-code session started{model_note}"))
                    .with_data(json!({
                        "sessionId": self.session_id,
                        "model": self.model,
                        "permissionMode": set(raw, "permissionMode"),
                        "apiKeySource": set(raw, "apiKeySource"),
                        "tools": raw.get("tools").and_then(Value::as_array).map(Vec::len),
                    })),
            ];
        }
        match subtype.and_then(Value::as_str) {
            Some(s @ ("api_retry" | "compact_boundary")) => {
                vec![AgentEvent::new(at, AgentEventType::Status).with_text(format!("claude-code {s}")).with_data(Value::Object(raw.clone()))]
            }
            Some(other) => {
                self.unknown_types.insert(format!("system:{other}"));
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    fn assistant(&mut self, raw: &Map<String, Value>, at: i64) -> Vec<AgentEvent> {
        let content = raw.get("message").and_then(as_record).and_then(|m| m.get("content")).and_then(Value::as_array);
        let Some(content) = content else { return Vec::new() };
        let mut events = Vec::new();
        for block in content {
            let Some(block) = block.as_object() else { continue };
            let block_type = str_of(block, "type");
            if block_type == Some("text") {
                if let Some(text) = str_of(block, "text") {
                    if !text.trim().is_empty() {
                        events.push(AgentEvent::new(at, AgentEventType::Message).with_text(text));
                    }
                }
            } else if block_type == Some("tool_use") {
                let name = str_of(block, "name").unwrap_or("unknown");
                events.push(
                    AgentEvent::new(at, AgentEventType::Tool)
                        .with_text(tool_summary(name, block.get("input")))
                        .with_data(json!({"phase": "call", "id": set(block, "id"), "name": name, "input": set(block, "input")})),
                );
            } else if let Some(t) = block_type {
                if t != "thinking" && t != "text" {
                    self.unknown_types.insert(format!("block:{t}"));
                }
            }
        }
        events
    }

    fn user(&mut self, raw: &Map<String, Value>, at: i64) -> Vec<AgentEvent> {
        let content = raw.get("message").and_then(as_record).and_then(|m| m.get("content")).and_then(Value::as_array);
        let Some(content) = content else { return Vec::new() };
        content
            .iter()
            .filter_map(Value::as_object)
            .filter(|b| str_of(b, "type") == Some("tool_result"))
            .map(|block| {
                let is_error = block.get("is_error") == Some(&Value::Bool(true));
                AgentEvent::new(at, AgentEventType::Tool)
                    .with_text(truncate(&block_text(block.get("content")), 300))
                    .with_data(json!({"phase": "result", "id": set(block, "tool_use_id"), "isError": is_error}))
            })
            .collect()
    }

    fn result_line(&mut self, raw: &Map<String, Value>, at: i64) -> Vec<AgentEvent> {
        let empty = Map::new();
        let u = raw.get("usage").and_then(as_record).unwrap_or(&empty);
        let cache_create = num(u.get("cache_creation_input_tokens"));
        let cache_read = num(u.get("cache_read_input_tokens"));
        let mut usage = AgentUsage {
            input_tokens: num(u.get("input_tokens")) + cache_create + cache_read,
            output_tokens: num(u.get("output_tokens")),
            cost_usd: None,
        };
        usage.cost_usd = raw.get("total_cost_usd").and_then(Value::as_f64).or_else(|| raw.get("cost_usd").and_then(Value::as_f64));
        let subtype = str_of(raw, "subtype").unwrap_or("success").to_string();
        let is_error = raw.get("is_error") == Some(&Value::Bool(true)) || subtype.starts_with("error");
        let result = ClaudeResult {
            subtype: subtype.clone(),
            is_error,
            text: str_of(raw, "result").unwrap_or("").to_string(),
            num_turns: num(raw.get("num_turns")),
            duration_ms: num(raw.get("duration_ms")),
            usage,
        };
        let mut events = vec![AgentEvent::new(at, AgentEventType::Usage).with_usage(usage).with_data(json!({
            "cacheCreationInputTokens": cache_create,
            "cacheReadInputTokens": cache_read,
            "numTurns": result.num_turns,
        }))];
        if is_error {
            let text = if result.text.is_empty() { format!("claude-code turn ended with {subtype}") } else { result.text.clone() };
            events.push(AgentEvent::new(at, AgentEventType::Error).with_text(text).with_data(json!({"subtype": subtype})));
        }
        self.result = Some(result);
        events
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClaudePermissionMode {
    Default,
    AcceptEdits,
    BypassPermissions,
    Plan,
    /// Any other mode the installed CLI understands, passed through as given.
    Other(String),
}

impl ClaudePermissionMode {
    pub fn as_str(&self) -> &str {
        match self {
            ClaudePermissionMode::Default => "default",
            ClaudePermissionMode::AcceptEdits => "acceptEdits",
            ClaudePermissionMode::BypassPermissions => "bypassPermissions",
            ClaudePermissionMode::Plan => "plan",
            ClaudePermissionMode::Other(s) => s,
        }
    }

    pub fn parse(s: &str) -> Self {
        match s {
            "default" => Self::Default,
            "acceptEdits" => Self::AcceptEdits,
            "bypassPermissions" => Self::BypassPermissions,
            "plan" => Self::Plan,
            other => Self::Other(other.to_string()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum PromptVia {
    #[default]
    Stdin,
    Arg,
}

#[derive(Clone, Default)]
pub struct ClaudeCodeOptions {
    pub cli: CliAgentOptions,
    /// `--permission-mode`. Default `acceptEdits`.
    pub permission_mode: Option<ClaudePermissionMode>,
    /// `--allowedTools` (comma-joined into one value). Default: the file tools plus Bash.
    pub allowed_tools: Option<Vec<String>>,
    pub disallowed_tools: Vec<String>,
    pub append_system_prompt: Option<String>,
    /// Deliver the prompt on stdin (default) or as the final argument.
    pub prompt_via: PromptVia,
    /// Set as `ANTHROPIC_API_KEY` for the child (otherwise inherited from `cli.env`).
    pub api_key: Option<String>,
    /// Skip the API-key preflight check (subscription login on the box; see docs/real-env/agents.md on terms).
    pub allow_subscription_auth: bool,
}

pub const DEFAULT_CLAUDE_TOOLS: [&str; 6] = ["Bash", "Read", "Edit", "Write", "Glob", "Grep"];

pub struct ClaudeCodeAdapter {
    cli: CliAgent,
    options: ClaudeCodeOptions,
}

impl ClaudeCodeAdapter {
    pub fn new(options: ClaudeCodeOptions) -> Self {
        let mut cli = CliAgent::new("claude", options.cli.clone());
        if let Some(key) = &options.api_key {
            cli.base_env.insert("ANTHROPIC_API_KEY".to_string(), key.clone());
        }
        Self { cli, options }
    }

    /// Argument list for one invocation (exported for tests and docs).
    pub fn build_args(&self, spec: &AgentSpec, resume_id: Option<&str>) -> Vec<String> {
        let o = &self.options;
        let mut args: Vec<String> = ["-p", "--output-format", "stream-json", "--verbose"].map(String::from).to_vec();
        args.extend(["--permission-mode".to_string(), o.permission_mode.as_ref().map_or("acceptEdits", ClaudePermissionMode::as_str).to_string()]);
        let allowed: Vec<String> = o.allowed_tools.clone().unwrap_or_else(|| DEFAULT_CLAUDE_TOOLS.map(String::from).to_vec());
        if !allowed.is_empty() {
            args.extend(["--allowedTools".to_string(), allowed.join(",")]);
        }
        if !o.disallowed_tools.is_empty() {
            args.extend(["--disallowedTools".to_string(), o.disallowed_tools.join(",")]);
        }
        if let Some(model) = spec.model.as_ref().filter(|m| !m.is_empty()) {
            args.extend(["--model".to_string(), model.clone()]);
        }
        if let Some(turns) = spec.max_turns.filter(|t| *t > 0) {
            args.extend(["--max-turns".to_string(), turns.to_string()]);
        }
        if let Some(prompt) = o.append_system_prompt.as_ref().filter(|p| !p.is_empty()) {
            args.extend(["--append-system-prompt".to_string(), prompt.clone()]);
        }
        if let Some(id) = resume_id.filter(|id| !id.is_empty()) {
            args.extend(["--resume".to_string(), id.to_string()]);
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
        on_event: EventCallback<'_>,
        cancel: &CancellationToken,
        is_start: bool,
    ) -> Result<(), AgentError> {
        let clock = self.cli.clock.clone();
        let mut parser = ClaudeStreamParser::new(move || clock.now());
        let via_arg = self.options.prompt_via == PromptVia::Arg;
        let mut final_args = args;
        if via_arg {
            final_args.extend(["--".to_string(), prompt.to_string()]);
        }
        let res = {
            let mut on_line = |line: &str| {
                for e in parser.parse_line(line) {
                    on_event(e);
                }
            };
            self.cli.run(&final_args, spec, (!via_arg).then(|| prompt.to_string()), &mut on_line, cancel).await
        };
        // A resumed session may come back under a new id; keep the handle current.
        if let Some(id) = parser.session_id.as_ref().filter(|id| !id.is_empty()) {
            session.id = id.clone();
        }

        let fail = |message: String| -> AgentError {
            on_event(
                AgentEvent::new(self.cli.clock.now(), AgentEventType::Error)
                    .with_text(message.clone())
                    .with_data(json!({"exitCode": res.code(), "stderr": res.stderr_tail})),
            );
            AgentError::Failed { message, exit_code: res.code(), stderr: Some(res.stderr_tail.clone()) }
        };

        if res.aborted {
            on_event(AgentEvent::new(self.cli.clock.now(), AgentEventType::Status).with_text("claude-code turn aborted"));
            return Err(AgentError::Aborted { session: (!session.id.is_empty()).then(|| session.clone()) });
        }
        if let Some(err) = res.spawn_error() {
            return Err(fail(format!("cannot run '{}': {err}", self.cli.binary)));
        }
        if let Some(result) = &parser.result {
            if result.is_error && result.subtype != "error_max_turns" {
                // the parser already emitted the error event
                let message = if result.text.is_empty() { format!("claude-code ended with {}", result.subtype) } else { result.text.clone() };
                return Err(AgentError::Failed { message, exit_code: res.code(), stderr: Some(res.stderr_tail.clone()) });
            }
        } else {
            let message = if res.code() != Some(0) {
                let how = res.signal().map(str::to_string).unwrap_or_else(|| format!("code {}", res.code().map_or("null".into(), |c| c.to_string())));
                let tail = if res.stderr_tail.is_empty() { String::new() } else { format!(": {}", res.stderr_tail) };
                format!("claude exited with {how}{tail}")
            } else {
                "claude stream ended without a result line".to_string()
            };
            return Err(fail(message));
        }
        if is_start && session.id.is_empty() {
            return Err(fail("claude did not report a session id".to_string()));
        }
        Ok(())
    }
}

#[async_trait]
impl AgentAdapter for ClaudeCodeAdapter {
    fn kind(&self) -> AgentKind {
        AgentKind::ClaudeCode
    }

    async fn preflight(&self) -> Preflight {
        let mut problems = Vec::new();
        if let Some(p) = self.cli.probe_binary().await {
            problems.push(p);
        }
        if !self.options.allow_subscription_auth && self.cli.base_env.get("ANTHROPIC_API_KEY").is_none_or(|k| k.is_empty()) {
            problems.push("ANTHROPIC_API_KEY is not set".to_string());
        }
        Preflight { ok: problems.is_empty(), problems }
    }

    async fn start(&self, spec: &AgentSpec, on_event: EventCallback<'_>, cancel: &CancellationToken) -> Result<AgentSession, AgentError> {
        let mut session = AgentSession { id: String::new(), kind: AgentKind::ClaudeCode };
        self.turn(&mut session, self.build_args(spec, None), spec, &spec.prompt, on_event, cancel, true).await?;
        Ok(session)
    }

    async fn resume(
        &self,
        session: &mut AgentSession,
        spec: &AgentSpec,
        message: &str,
        on_event: EventCallback<'_>,
        cancel: &CancellationToken,
    ) -> Result<(), AgentError> {
        let args = self.build_args(spec, Some(&session.id));
        self.turn(session, args, spec, message, on_event, cancel, false).await
    }
}
