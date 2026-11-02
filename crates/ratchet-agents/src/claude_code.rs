//! Claude Code adapter: spawns `claude -p --output-format stream-json --verbose` and maps its stream to `AgentEvent`s.

use std::collections::BTreeSet;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tokio::sync::mpsc;

use crate::classify::{FailureKind, FailureOnce, classify_status, classify_text};
use crate::cli_agent::{CliAgent, CliAgentOptions, as_record, build_env, num, str_of, truncate};
use crate::detect::HarnessKind;
use crate::harness::{CLOSE_GRACE, Scratch, TurnRunner, open_with_fallback};
use crate::live::LiveProcess;
use crate::models::resolve_model;
use crate::process::{LineSplitter, ProcessExit, SpawnOptions};
use ratchet_core::enums::AgentKind;
use ratchet_core::ports::{
    Access, AgentAdapter, AgentEvent, AgentEventType, AgentSession, AgentSpec, AgentUsage,
    CancellationToken, Capabilities, EventSink, Harness, LiveSession, McpServer, PortError,
    PortResult, Preflight, SessionSpec, SharedEventSink,
};

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
        for key in [
            "command",
            "file_path",
            "path",
            "pattern",
            "url",
            "description",
        ] {
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
    /// How the last finished turn failed, when it was a rate limit or an expired login (cleared when the next turn's first line
    /// arrives, so a live session reads it between turns).
    pub failure: Option<FailureKind>,
    splitter: LineSplitter,
    turn_failure: FailureOnce,
    turn_over: bool,
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
            failure: None,
            splitter: LineSplitter::new(),
            turn_failure: FailureOnce::default(),
            turn_over: false,
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
        if let Some(id) = str_of(obj, "session_id")
            && self.session_id.as_deref().is_none_or(str::is_empty)
        {
            self.session_id = Some(id.to_string());
        }
        let at = (self.now)();
        if self.turn_over {
            // the previous turn of a live session is over; this line starts the next
            self.turn_over = false;
            self.failure = None;
            self.turn_failure.reset();
        }
        match kind {
            "system" => self.system(obj, at),
            "assistant" => self.assistant(obj, at),
            "user" => self.user(obj, at),
            "result" => self.result_line(obj, at),
            "stream_event" => self.stream_event(obj, at),
            "rate_limit_event" => self.rate_limit_event(obj, at),
            other => {
                self.unknown_types.insert(other.to_string());
                Vec::new()
            }
        }
    }

    /// `--include-partial-messages`: Anthropic stream events; text and thinking deltas are reported, the rest is bookkeeping.
    fn stream_event(&mut self, raw: &Map<String, Value>, at: i64) -> Vec<AgentEvent> {
        let Some(delta) = raw
            .get("event")
            .and_then(as_record)
            .filter(|e| str_of(e, "type") == Some("content_block_delta"))
            .and_then(|e| e.get("delta"))
            .and_then(as_record)
        else {
            return Vec::new();
        };
        match str_of(delta, "type") {
            Some("text_delta") => str_of(delta, "text")
                .filter(|t| !t.is_empty())
                .map(|t| vec![AgentEvent::new(at, AgentEventType::Delta).with_text(t)])
                .unwrap_or_default(),
            Some("thinking_delta") => str_of(delta, "thinking")
                .filter(|t| !t.is_empty())
                .map(|t| vec![AgentEvent::new(at, AgentEventType::Thinking).with_text(t)])
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    /// `status: "rejected"` is the quota being out (with the structured reset time); `allowed` / `allowed_warning` are only
    /// information.
    fn rate_limit_event(&mut self, raw: &Map<String, Value>, at: i64) -> Vec<AgentEvent> {
        let Some(info) = raw.get("rate_limit_info").and_then(as_record) else {
            return Vec::new();
        };
        if str_of(info, "status") != Some("rejected") {
            return Vec::new();
        }
        let resets_at = info.get("resetsAt").and_then(Value::as_i64);
        let kind = str_of(info, "rateLimitType").unwrap_or("quota");
        self.signal(
            at,
            FailureKind::RateLimited { resets_at },
            &format!("claude-code {kind} limit reached"),
        )
    }

    /// The first signal of a turn wins: the `rate_limit_event` carries the reset time that the assistant line and the result lack.
    fn signal(&mut self, at: i64, kind: FailureKind, text: &str) -> Vec<AgentEvent> {
        let event = self.turn_failure.emit(at, kind, text);
        self.failure = self.turn_failure.kind();
        event.into_iter().collect()
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
            let model_note = self
                .model
                .as_deref()
                .filter(|m| !m.is_empty())
                .map(|m| format!(" ({m})"))
                .unwrap_or_default();
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
                vec![
                    AgentEvent::new(at, AgentEventType::Status)
                        .with_text(format!("claude-code {s}"))
                        .with_data(Value::Object(raw.clone())),
                ]
            }
            Some(other) => {
                self.unknown_types.insert(format!("system:{other}"));
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    fn assistant(&mut self, raw: &Map<String, Value>, at: i64) -> Vec<AgentEvent> {
        let content = raw
            .get("message")
            .and_then(as_record)
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array);
        let mut events = Vec::new();
        // `error: "rate_limit" | "authentication_failed"` marks an assistant line that carries the CLI's failure text.
        let by_error = match str_of(raw, "error") {
            Some("rate_limit") => Some(FailureKind::RateLimited { resets_at: None }),
            Some("authentication_failed") => Some(FailureKind::AuthExpired),
            _ => None,
        };
        if let Some(kind) = by_error {
            let text = content
                .map(|c| block_text(Some(&Value::Array(c.clone()))))
                .unwrap_or_default();
            events.extend(self.signal(at, kind, &text));
        }
        let Some(content) = content else {
            return events;
        };
        for block in content {
            let Some(block) = block.as_object() else {
                continue;
            };
            let block_type = str_of(block, "type");
            if block_type == Some("text") {
                if let Some(text) = str_of(block, "text")
                    && !text.trim().is_empty()
                {
                    events.push(AgentEvent::new(at, AgentEventType::Message).with_text(text));
                }
            } else if block_type == Some("tool_use") {
                let name = str_of(block, "name").unwrap_or("unknown");
                events.push(
                    AgentEvent::new(at, AgentEventType::Tool)
                        .with_text(tool_summary(name, block.get("input")))
                        .with_data(json!({"phase": "call", "id": set(block, "id"), "name": name, "input": set(block, "input")})),
                );
            } else if let Some(t) = block_type
                && t != "thinking"
                && t != "text"
            {
                self.unknown_types.insert(format!("block:{t}"));
            }
        }
        events
    }

    fn user(&mut self, raw: &Map<String, Value>, at: i64) -> Vec<AgentEvent> {
        let content = raw
            .get("message")
            .and_then(as_record)
            .and_then(|m| m.get("content"))
            .and_then(Value::as_array);
        let Some(content) = content else {
            return Vec::new();
        };
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
        usage.cost_usd = raw
            .get("total_cost_usd")
            .and_then(Value::as_f64)
            .or_else(|| raw.get("cost_usd").and_then(Value::as_f64));
        let subtype = str_of(raw, "subtype").unwrap_or("success").to_string();
        let is_error =
            raw.get("is_error") == Some(&Value::Bool(true)) || subtype.starts_with("error");
        let result = ClaudeResult {
            subtype: subtype.clone(),
            is_error,
            text: str_of(raw, "result").unwrap_or("").to_string(),
            num_turns: num(raw.get("num_turns")),
            duration_ms: num(raw.get("duration_ms")),
            usage,
        };
        let mut events = vec![
            AgentEvent::new(at, AgentEventType::Usage)
                .with_usage(usage)
                .with_data(json!({
                    "cacheCreationInputTokens": cache_create,
                    "cacheReadInputTokens": cache_read,
                    "numTurns": result.num_turns,
                })),
        ];
        if is_error {
            let text = if result.text.is_empty() {
                format!("claude-code turn ended with {subtype}")
            } else {
                result.text.clone()
            };
            events.push(
                AgentEvent::new(at, AgentEventType::Error)
                    .with_text(text.clone())
                    .with_data(json!({"subtype": subtype})),
            );
            let kind = raw
                .get("api_error_status")
                .and_then(Value::as_i64)
                .and_then(classify_status)
                .or_else(|| classify_text(&text, at / 1000));
            if let Some(kind) = kind {
                events.extend(self.signal(at, kind, &text));
            }
        }
        self.result = Some(result);
        self.turn_over = true;
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
    /// `--fallback-model`: the CLI's own retry on another model when the first is overloaded.
    pub fallback_model: Option<String>,
}

pub const DEFAULT_CLAUDE_TOOLS: [&str; 6] = ["Bash", "Read", "Edit", "Write", "Glob", "Grep"];

/// The tools a read-only session keeps (plus its MCP servers, as `mcp__<server>`).
pub const READ_ONLY_CLAUDE_TOOLS: [&str; 3] = ["Read", "Glob", "Grep"];

/// Tools a read-only session must not have, so a user setting that allows them cannot widen the session.
const WRITE_CLAUDE_TOOLS: [&str; 5] = ["Bash", "Edit", "Write", "MultiEdit", "NotebookEdit"];

/// The `--mcp-config` file: `{"mcpServers": {name: {type, command, args, env}}}`.
pub fn claude_mcp_config(servers: &[McpServer]) -> Value {
    let mut map = Map::new();
    for s in servers {
        map.insert(
            s.name.clone(),
            json!({"type": "stdio", "command": s.command, "args": s.args, "env": s.env}),
        );
    }
    json!({ "mcpServers": map })
}

#[derive(Clone)]
pub struct ClaudeCodeAdapter {
    cli: CliAgent,
    options: ClaudeCodeOptions,
}

impl ClaudeCodeAdapter {
    pub fn new(options: ClaudeCodeOptions) -> Self {
        let mut cli = CliAgent::new("claude", options.cli.clone());
        if let Some(key) = &options.api_key {
            cli.base_env
                .insert("ANTHROPIC_API_KEY".to_string(), key.clone());
        }
        Self { cli, options }
    }

    /// Argument list for one invocation (exported for tests and docs).
    pub fn build_args(&self, spec: &AgentSpec, resume_id: Option<&str>) -> Vec<String> {
        self.build_session_args(&SessionSpec::from(spec), resume_id, None)
    }

    /// Flags shared by one-shot and live runs. `mcp_config` is the path of the written [`claude_mcp_config`] file.
    fn session_flags(
        &self,
        spec: &SessionSpec,
        resume_id: Option<&str>,
        mcp_config: Option<&Path>,
    ) -> Vec<String> {
        let o = &self.options;
        let read_only = spec.access == Access::ReadOnly;
        let mut args = Vec::new();
        args.extend([
            "--permission-mode".to_string(),
            if read_only {
                // anything that would need approval is denied, never asked (nobody is there to answer)
                "dontAsk".to_string()
            } else {
                o.permission_mode
                    .as_ref()
                    .map_or("acceptEdits", ClaudePermissionMode::as_str)
                    .to_string()
            },
        ]);
        let mut allowed: Vec<String> = if read_only {
            READ_ONLY_CLAUDE_TOOLS.map(String::from).to_vec()
        } else {
            o.allowed_tools
                .clone()
                .unwrap_or_else(|| DEFAULT_CLAUDE_TOOLS.map(String::from).to_vec())
        };
        if read_only {
            allowed.retain(|t| !WRITE_CLAUDE_TOOLS.contains(&t.as_str()));
        }
        allowed.extend(spec.mcp.iter().map(|m| format!("mcp__{}", m.name)));
        if !allowed.is_empty() {
            args.extend(["--allowedTools".to_string(), allowed.join(",")]);
        }
        let mut disallowed = o.disallowed_tools.clone();
        if read_only {
            disallowed.extend(WRITE_CLAUDE_TOOLS.map(String::from));
        }
        if matches!(spec.access, Access::Edit { network: false }) {
            disallowed.extend(["WebFetch".to_string(), "WebSearch".to_string()]);
        }
        if !disallowed.is_empty() {
            args.extend(["--disallowedTools".to_string(), disallowed.join(",")]);
        }
        if let Some(model) = spec.model.as_ref().filter(|m| !m.is_empty()) {
            let model = &resolve_model(HarnessKind::ClaudeCode, model).id;
            args.extend(["--model".to_string(), model.clone()]);
        }
        if let Some(turns) = spec.max_turns.filter(|t| *t > 0) {
            args.extend(["--max-turns".to_string(), turns.to_string()]);
        }
        if let Some(model) = o.fallback_model.as_ref().filter(|m| !m.is_empty()) {
            args.extend(["--fallback-model".to_string(), model.clone()]);
        }
        if let Some(path) = mcp_config {
            // strict: the user's own global MCP servers are not loaded into a Ratchet session
            args.extend([
                "--mcp-config".to_string(),
                path.to_string_lossy().into_owned(),
                "--strict-mcp-config".to_string(),
            ]);
        }
        let append: Vec<&str> = [
            o.append_system_prompt.as_deref(),
            spec.system_append.as_deref(),
        ]
        .into_iter()
        .flatten()
        .filter(|p| !p.is_empty())
        .collect();
        if !append.is_empty() {
            args.extend(["--append-system-prompt".to_string(), append.join("\n\n")]);
        }
        if let Some(id) = resume_id.filter(|id| !id.is_empty()) {
            args.extend(["--resume".to_string(), id.to_string()]);
        }
        args.extend(self.cli.extra_args.iter().cloned());
        args
    }

    /// Argument list of a one-shot turn for `spec`: [`ClaudeCodeAdapter::build_args`] plus access, MCP servers and the system
    /// prompt add-on.
    pub fn build_session_args(
        &self,
        spec: &SessionSpec,
        resume_id: Option<&str>,
        mcp_config: Option<&Path>,
    ) -> Vec<String> {
        let mut args: Vec<String> = ["-p", "--output-format", "stream-json", "--verbose"]
            .map(String::from)
            .to_vec();
        args.extend(self.session_flags(spec, resume_id, mcp_config));
        args
    }

    /// Argument list of a live session: messages arrive on stdin as stream-json, text as partial-message deltas.
    pub fn build_live_args(
        &self,
        spec: &SessionSpec,
        resume_id: Option<&str>,
        mcp_config: Option<&Path>,
    ) -> Vec<String> {
        let mut args: Vec<String> = [
            "-p",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--verbose",
            "--include-partial-messages",
            "--replay-user-messages",
        ]
        .map(String::from)
        .to_vec();
        args.extend(self.session_flags(spec, resume_id, mcp_config));
        args
    }

    /// The `--mcp-config` file for `spec`, when it has MCP servers. Keep the scratch alive while the CLI runs.
    fn write_mcp_config(
        spec: &SessionSpec,
        scratch: &Scratch,
    ) -> PortResult<Option<std::path::PathBuf>> {
        if spec.mcp.is_empty() {
            return Ok(None);
        }
        scratch
            .write("mcp.json", &claude_mcp_config(&spec.mcp).to_string())
            .map(Some)
    }

    /// Run the first turn of a new session (or, with `spec.resume`, the next turn of an earlier one) with everything `spec`
    /// asks for. `AgentAdapter::start` is this with the defaults of [`SessionSpec::from`].
    pub async fn start_session(
        &self,
        spec: &SessionSpec,
        on_event: &EventSink<'_>,
        cancel: Option<&CancellationToken>,
    ) -> PortResult<AgentSession> {
        let cancel = cancel.cloned().unwrap_or_default();
        let resume = spec.resume.clone().filter(|r| !r.is_empty());
        let mut session = AgentSession {
            id: resume.clone().unwrap_or_default(),
            kind: AgentKind::ClaudeCode,
        };
        self.turn(
            &mut session,
            spec,
            resume.as_deref(),
            &spec.prompt,
            on_event,
            &cancel,
            resume.is_none(),
        )
        .await?;
        Ok(session)
    }

    /// `AgentAdapter::resume` with a [`SessionSpec`].
    pub async fn resume_session(
        &self,
        session: &mut AgentSession,
        spec: &SessionSpec,
        message: &str,
        on_event: &EventSink<'_>,
        cancel: Option<&CancellationToken>,
    ) -> PortResult<()> {
        let cancel = cancel.cloned().unwrap_or_default();
        let id = session.id.clone();
        self.turn(session, spec, Some(&id), message, on_event, &cancel, false)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn turn(
        &self,
        session: &mut AgentSession,
        spec: &SessionSpec,
        resume_id: Option<&str>,
        prompt: &str,
        on_event: &EventSink<'_>,
        cancel: &CancellationToken,
        is_start: bool,
    ) -> PortResult<()> {
        let scratch = Scratch::new("claude")?;
        let mcp = Self::write_mcp_config(spec, &scratch)?;
        let args = self.build_session_args(spec, resume_id, mcp.as_deref());
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
            self.cli
                .run_in(
                    &final_args,
                    &spec.workdir,
                    spec.env.as_ref(),
                    (!via_arg).then(|| prompt.to_string()),
                    &mut on_line,
                    cancel,
                )
                .await
        };
        drop(scratch);
        // A resumed session may come back under a new id; keep the handle current.
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
                    .with_text("claude-code turn aborted"),
            );
            return Err(PortError::Cancelled);
        }
        if let Some(err) = res.spawn_error() {
            return Err(fail(format!("cannot run '{}': {err}", self.cli.binary)));
        }
        if let Some(result) = &parser.result {
            if result.is_error && result.subtype != "error_max_turns" {
                // the parser already emitted the error event
                let message = if result.text.is_empty() {
                    format!("claude-code ended with {}", result.subtype)
                } else {
                    result.text.clone()
                };
                return Err(PortError::AgentFailed {
                    message,
                    exit_code: res.code(),
                    stderr: Some(res.stderr_tail.clone()),
                });
            }
        } else {
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
impl TurnRunner for ClaudeCodeAdapter {
    fn kind(&self) -> AgentKind {
        AgentKind::ClaudeCode
    }

    async fn run_turn(
        &self,
        session: &mut AgentSession,
        spec: &SessionSpec,
        resume: Option<&str>,
        prompt: &str,
        events: &EventSink<'_>,
        cancel: &CancellationToken,
    ) -> PortResult<()> {
        self.turn(
            session,
            spec,
            resume,
            prompt,
            events,
            cancel,
            resume.is_none(),
        )
        .await
    }
}

#[async_trait]
impl AgentAdapter for ClaudeCodeAdapter {
    fn kind(&self) -> AgentKind {
        AgentKind::ClaudeCode
    }

    async fn preflight(&self) -> PortResult<Preflight> {
        let mut problems = Vec::new();
        if let Some(p) = self.cli.probe_binary().await {
            problems.push(p);
        }
        if !self.options.allow_subscription_auth
            && self
                .cli
                .base_env
                .get("ANTHROPIC_API_KEY")
                .is_none_or(|k| k.is_empty())
        {
            problems.push("ANTHROPIC_API_KEY is not set".to_string());
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
        self.start_session(&SessionSpec::from(spec), on_event, cancel)
            .await
    }

    async fn resume(
        &self,
        session: &mut AgentSession,
        spec: &AgentSpec,
        message: &str,
        on_event: &EventSink<'_>,
        cancel: Option<&CancellationToken>,
    ) -> PortResult<()> {
        self.resume_session(session, &SessionSpec::from(spec), message, on_event, cancel)
            .await
    }
}

#[async_trait]
impl Harness for ClaudeCodeAdapter {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            streaming: true,
            live_session: !self.cli.disable_live_session,
            mcp: true,
            cost_in_usd: true,
            read_only: true,
        }
    }

    async fn open(
        &self,
        spec: &SessionSpec,
        events: SharedEventSink,
    ) -> PortResult<Box<dyn LiveSession>> {
        let runner: Arc<dyn TurnRunner> = Arc::new(self.clone());
        let clock = self.cli.clock.clone();
        open_with_fallback(
            self.capabilities().live_session,
            move || clock.now(),
            "claude-code",
            runner,
            spec,
            events.clone(),
            || async { ClaudeLiveSession::open(self, spec, events).await },
        )
        .await
    }
}

// ---------------------------------------------------------------- live session

/// What the reader task tells `send` about the turn.
enum Turn {
    /// A `result` line arrived.
    Done { is_error: bool, message: String },
    /// The process is gone.
    Exited(String),
}

/// `claude -p --input-format stream-json --output-format stream-json --include-partial-messages --replay-user-messages`: one
/// process for the whole chat. Each user message is one stdin line; a turn ends at its `result` line.
pub struct ClaudeLiveSession {
    process: LiveProcess,
    turns: tokio::sync::Mutex<mpsc::UnboundedReceiver<Turn>>,
    session_id: Arc<std::sync::Mutex<Option<String>>>,
    interrupted: Arc<AtomicBool>,
    interrupt_note: tokio::sync::Notify,
    busy: AtomicBool,
    events: SharedEventSink,
    clock: Arc<dyn ratchet_core::ports::Clock>,
    _scratch: Scratch,
}

/// After an interrupt the CLI should end the turn with a `result`; if it does not, the turn is over after this long anyway.
const INTERRUPT_WAIT: Duration = Duration::from_secs(5);

impl ClaudeLiveSession {
    async fn open(
        adapter: &ClaudeCodeAdapter,
        spec: &SessionSpec,
        events: SharedEventSink,
    ) -> PortResult<Box<dyn LiveSession>> {
        let scratch = Scratch::new("claude-live")?;
        let mcp = ClaudeCodeAdapter::write_mcp_config(spec, &scratch)?;
        let resume = spec.resume.clone().filter(|r| !r.is_empty());
        let args = adapter.build_live_args(spec, resume.as_deref(), mcp.as_deref());
        let (tx, rx) = mpsc::unbounded_channel();
        let session_id = Arc::new(std::sync::Mutex::new(resume));
        let clock = adapter.cli.clock.clone();
        let mut parser = ClaudeStreamParser::new({
            let clock = clock.clone();
            move || clock.now()
        });
        let line_tx = tx.clone();
        let line_id = session_id.clone();
        let line_events = events.clone();
        let interrupted = Arc::new(AtomicBool::new(false));
        let line_interrupted = interrupted.clone();
        let process = LiveProcess::spawn(
            adapter.cli.spawner.as_ref(),
            &adapter.cli.binary,
            &args,
            &SpawnOptions {
                cwd: spec.workdir.clone().into(),
                env: build_env(&adapter.cli.base_env, spec.env.as_ref()),
            },
            move |_, line| {
                for e in parser.parse_line(line) {
                    // the user stopped this turn: the CLI's "interrupted" result is not an error to show
                    if line_interrupted.load(Ordering::SeqCst)
                        && e.event_type == AgentEventType::Error
                    {
                        continue;
                    }
                    line_events(e);
                }
                if let Some(id) = parser.session_id.clone() {
                    *line_id.lock().unwrap() = Some(id);
                }
                if let Some(result) = parser.result.take() {
                    let _ = line_tx.send(Turn::Done {
                        is_error: result.is_error && result.subtype != "error_max_turns",
                        message: if result.text.is_empty() {
                            format!("claude-code ended with {}", result.subtype)
                        } else {
                            result.text
                        },
                    });
                }
            },
            {
                let tx = tx.clone();
                move |exit| {
                    let _ = tx.send(Turn::Exited(
                        exit.spawn_error
                            .clone()
                            .unwrap_or_else(|| format!("exited with {}", exit_text(exit))),
                    ));
                }
            },
        );
        // A binary that cannot start shows up at once; let the caller fall back to per-turn mode.
        process.wait_exit(Duration::from_millis(100)).await;
        if let Some(exit) = process.exit() {
            return Err(PortError::AgentFailed {
                message: format!(
                    "claude live session did not start: {}",
                    exit.spawn_error.clone().unwrap_or_else(|| exit_text(&exit))
                ),
                exit_code: exit.code,
                stderr: Some(process.stderr_tail()),
            });
        }
        Ok(Box::new(Self {
            process,
            turns: tokio::sync::Mutex::new(rx),
            session_id,
            interrupted,
            interrupt_note: tokio::sync::Notify::new(),
            busy: AtomicBool::new(false),
            events,
            clock,
            _scratch: scratch,
        }))
    }
}

fn exit_text(exit: &ProcessExit) -> String {
    exit.signal.clone().unwrap_or_else(|| {
        format!(
            "code {}",
            exit.code.map_or("null".into(), |c| c.to_string())
        )
    })
}

#[async_trait]
impl LiveSession for ClaudeLiveSession {
    async fn send(&self, text: &str) -> PortResult<()> {
        let mut turns = self.turns.lock().await;
        while turns.try_recv().is_ok() {}
        self.interrupted.store(false, Ordering::SeqCst);
        self.busy.store(true, Ordering::SeqCst);
        self.process.write_json(&json!({
            "type": "user",
            "message": {"role": "user", "content": text},
        }));
        let after_interrupt = async {
            self.interrupt_note.notified().await;
            tokio::time::sleep(INTERRUPT_WAIT).await;
        };
        let outcome = tokio::select! {
            o = turns.recv() => o,
            () = after_interrupt => None,
        };
        self.busy.store(false, Ordering::SeqCst);
        let interrupted = self.interrupted.load(Ordering::SeqCst);
        let failed = |message: String| {
            (self.events)(
                AgentEvent::new(self.clock.now(), AgentEventType::Error)
                    .with_text(message.clone())
                    .with_data(json!({"stderr": self.process.stderr_tail()})),
            );
            Err(PortError::AgentFailed {
                message,
                exit_code: self.process.exit().and_then(|e| e.code),
                stderr: Some(self.process.stderr_tail()),
            })
        };
        match outcome {
            Some(Turn::Done { is_error, message }) => {
                if is_error && !interrupted {
                    // the parser already reported the error (and its classification)
                    Err(PortError::AgentFailed {
                        message,
                        exit_code: None,
                        stderr: Some(self.process.stderr_tail()),
                    })
                } else {
                    Ok(())
                }
            }
            _ if interrupted => Ok(()),
            Some(Turn::Exited(how)) => failed(format!("claude {how}")),
            None => failed("claude live session ended".to_string()),
        }
    }

    async fn interrupt(&self) -> PortResult<()> {
        if !self.busy.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.interrupted.store(true, Ordering::SeqCst);
        self.process.write_json(&json!({
            "type": "control_request",
            "request_id": "ratchet-interrupt",
            "request": {"subtype": "interrupt"},
        }));
        self.interrupt_note.notify_one();
        Ok(())
    }

    fn session_id(&self) -> Option<String> {
        self.session_id.lock().unwrap().clone()
    }

    async fn close(self: Box<Self>) {
        self.process.close(CLOSE_GRACE).await;
    }
}
