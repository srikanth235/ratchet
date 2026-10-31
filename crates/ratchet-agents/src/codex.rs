//! Codex adapter: spawns `codex exec --json` and maps its JSONL to `AgentEvent`s.

use std::collections::{BTreeSet, HashSet};

use async_trait::async_trait;
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use crate::claude_code::PromptVia;
use crate::cli_agent::{CliAgent, CliAgentOptions, as_record, num, str_of, truncate};
use crate::domain::{
    AgentAdapter, AgentError, AgentEvent, AgentEventType, AgentKind, AgentSession, AgentSpec,
    AgentUsage, EventCallback, Preflight,
};
use crate::process::LineSplitter;

/// USD per million tokens, for estimating cost: `codex exec --json` reports tokens but no dollars.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CodexPricing {
    pub input_per_m: f64,
    pub cached_input_per_m: Option<f64>,
    pub output_per_m: f64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct CodexTurnResult {
    pub completed: bool,
    pub failed: bool,
    pub error: Option<String>,
    pub usage: Option<AgentUsage>,
}

/// Incremental parser for `codex exec --json` JSONL. Same tolerance rules as the Claude parser. Usage is reported once per turn,
/// from `turn.completed`.
pub struct CodexStreamParser {
    pub thread_id: Option<String>,
    pub turn: CodexTurnResult,
    pub malformed_lines: usize,
    pub unknown_types: BTreeSet<String>,
    splitter: LineSplitter,
    started: HashSet<String>,
    last_error: Option<String>,
    now: Box<dyn Fn() -> i64 + Send + Sync>,
    pricing: Option<CodexPricing>,
}

impl CodexStreamParser {
    pub fn new(
        now: impl Fn() -> i64 + Send + Sync + 'static,
        pricing: Option<CodexPricing>,
    ) -> Self {
        Self {
            thread_id: None,
            turn: CodexTurnResult::default(),
            malformed_lines: 0,
            unknown_types: BTreeSet::new(),
            splitter: LineSplitter::new(),
            started: HashSet::new(),
            last_error: None,
            now: Box::new(now),
            pricing,
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
        let at = (self.now)();
        match kind {
            "thread.started" => {
                if let Some(id) = str_of(obj, "thread_id") {
                    self.thread_id = Some(id.to_string());
                }
                vec![
                    AgentEvent::new(at, AgentEventType::Status)
                        .with_text("codex thread started")
                        .with_data(json!({"sessionId": self.thread_id})),
                ]
            }
            "turn.started" => Vec::new(),
            "item.started" | "item.updated" => match obj.get("item").and_then(as_record) {
                Some(item) => self.item(item, at, kind == "item.started"),
                None => Vec::new(),
            },
            "item.completed" => match obj.get("item").and_then(as_record) {
                Some(item) => self.item(item, at, false),
                None => Vec::new(),
            },
            "turn.completed" => self.turn_completed(obj, at),
            "turn.failed" => {
                let message = obj
                    .get("error")
                    .and_then(as_record)
                    .and_then(|e| str_of(e, "message"))
                    .map(str::to_string)
                    .or_else(|| self.last_error.clone())
                    .unwrap_or_else(|| "codex turn failed".to_string());
                self.turn = CodexTurnResult {
                    completed: false,
                    failed: true,
                    error: Some(message.clone()),
                    usage: None,
                };
                vec![
                    AgentEvent::new(at, AgentEventType::Error)
                        .with_text(message)
                        .with_data(json!({"fatal": true})),
                ]
            }
            "error" => {
                // Often a transient stream error (reconnecting); `turn.failed` follows when it is fatal.
                let message = str_of(obj, "message").unwrap_or("codex error").to_string();
                self.last_error = Some(message.clone());
                vec![
                    AgentEvent::new(at, AgentEventType::Error)
                        .with_text(message)
                        .with_data(json!({"fatal": false})),
                ]
            }
            other => {
                self.unknown_types.insert(other.to_string());
                Vec::new()
            }
        }
    }

    fn item(&mut self, item: &Map<String, Value>, at: i64, is_start: bool) -> Vec<AgentEvent> {
        let id = str_of(item, "id");
        let kind = str_of(item, "type");
        let id_value = id.map_or(Value::Null, |i| Value::String(i.to_string()));
        match kind {
            Some("agent_message") => match str_of(item, "text") {
                Some(text) if !text.trim().is_empty() => {
                    vec![AgentEvent::new(at, AgentEventType::Message).with_text(text)]
                }
                _ => Vec::new(),
            },
            Some("command_execution") => {
                let command = str_of(item, "command").unwrap_or("");
                let mut events = Vec::new();
                if let Some(id) = id.filter(|i| !i.is_empty())
                    && self.started.insert(id.to_string())
                {
                    events.push(
                            AgentEvent::new(at, AgentEventType::Tool)
                                .with_text(format!("command: {}", truncate(command, 200)))
                                .with_data(json!({"phase": "call", "id": id, "name": "command", "input": {"command": command}})),
                        );
                }
                if !is_start && str_of(item, "status") != Some("in_progress") {
                    let exit_code = item
                        .get("exit_code")
                        .and_then(Value::as_f64)
                        .map(|n| n as i64);
                    events.push(
                        AgentEvent::new(at, AgentEventType::Tool)
                            .with_text(truncate(
                                str_of(item, "aggregated_output").unwrap_or(""),
                                300,
                            ))
                            .with_data(json!({
                                "phase": "result",
                                "id": id_value,
                                "exitCode": exit_code,
                                "isError": exit_code.is_some_and(|c| c != 0),
                            })),
                    );
                }
                events
            }
            Some("file_change") => {
                if is_start {
                    return Vec::new();
                }
                let changes: Vec<Value> = item
                    .get("changes")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
                let paths: Vec<&str> = changes
                    .iter()
                    .filter_map(|c| c.as_object().and_then(|o| str_of(o, "path")))
                    .collect();
                vec![
                    AgentEvent::new(at, AgentEventType::Tool)
                        .with_text(format!("apply_patch: {}", paths.join(", ")))
                        .with_data(json!({"phase": "call", "id": id_value, "name": "apply_patch", "input": {"changes": changes}})),
                ]
            }
            Some(k @ ("mcp_tool_call" | "web_search")) => {
                if is_start {
                    return Vec::new();
                }
                let name = if k == "web_search" {
                    "web_search".to_string()
                } else {
                    let display = |key: &str, default: &str| match item.get(key) {
                        None | Some(Value::Null) => default.to_string(),
                        Some(Value::String(s)) => s.clone(),
                        Some(other) => other.to_string(),
                    };
                    format!("{}.{}", display("server", "mcp"), display("tool", ""))
                };
                vec![
                    AgentEvent::new(at, AgentEventType::Tool)
                        .with_text(name.clone())
                        .with_data(json!({"phase": "call", "id": id_value, "name": name})),
                ]
            }
            Some("error") => match str_of(item, "message") {
                Some(message) => vec![
                    AgentEvent::new(at, AgentEventType::Error)
                        .with_text(message)
                        .with_data(json!({"fatal": false})),
                ],
                None => Vec::new(),
            },
            Some("reasoning" | "todo_list") => Vec::new(),
            Some(other) => {
                self.unknown_types.insert(format!("item:{other}"));
                Vec::new()
            }
            None => Vec::new(),
        }
    }

    fn turn_completed(&mut self, raw: &Map<String, Value>, at: i64) -> Vec<AgentEvent> {
        let empty = Map::new();
        let u = raw.get("usage").and_then(as_record).unwrap_or(&empty);
        let cached = num(u.get("cached_input_tokens"));
        let mut usage = AgentUsage {
            input_tokens: num(u.get("input_tokens")),
            output_tokens: num(u.get("output_tokens")),
            cost_usd: None,
        };
        if let Some(p) = &self.pricing {
            let uncached = usage.input_tokens.saturating_sub(cached) as f64;
            usage.cost_usd = Some(
                (uncached * p.input_per_m
                    + cached as f64 * p.cached_input_per_m.unwrap_or(p.input_per_m)
                    + usage.output_tokens as f64 * p.output_per_m)
                    / 1e6,
            );
        }
        self.turn = CodexTurnResult {
            completed: true,
            failed: false,
            error: None,
            usage: Some(usage),
        };
        vec![
            AgentEvent::new(at, AgentEventType::Usage)
                .with_usage(usage)
                .with_data(json!({"cachedInputTokens": cached})),
        ]
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexSandbox {
    ReadOnly,
    WorkspaceWrite,
    DangerFullAccess,
}

impl CodexSandbox {
    pub fn as_str(self) -> &'static str {
        match self {
            CodexSandbox::ReadOnly => "read-only",
            CodexSandbox::WorkspaceWrite => "workspace-write",
            CodexSandbox::DangerFullAccess => "danger-full-access",
        }
    }
}

#[derive(Clone, Default)]
pub struct CodexOptions {
    pub cli: CliAgentOptions,
    /// Sandbox mode passed as `-c sandbox_mode=...`. Default `workspace-write`.
    pub sandbox: Option<CodexSandbox>,
    /// Allow network inside the workspace-write sandbox. Default true (agents run experiments, pnpm, aws).
    pub network_access: Option<bool>,
    /// Extra `-c key=value` overrides.
    pub config_overrides: Vec<String>,
    pub pricing: Option<CodexPricing>,
    /// Deliver the prompt on stdin via `-` (default) or as the final argument.
    pub prompt_via: PromptVia,
    /// Set as `OPENAI_API_KEY` (and `CODEX_API_KEY`) for the child.
    pub api_key: Option<String>,
    /// Skip the API-key preflight check: the CLI is signed in with a ChatGPT account (`codex login`) on this machine.
    pub allow_subscription_auth: bool,
}

pub struct CodexAdapter {
    cli: CliAgent,
    options: CodexOptions,
}

impl CodexAdapter {
    pub fn new(options: CodexOptions) -> Self {
        let mut cli = CliAgent::new("codex", options.cli.clone());
        if let Some(key) = &options.api_key {
            cli.base_env
                .insert("OPENAI_API_KEY".to_string(), key.clone());
        }
        Self { cli, options }
    }

    /// Argument list for one invocation, excluding the prompt (exported for tests and docs).
    pub fn build_args(&self, spec: &AgentSpec, resume_id: Option<&str>) -> Vec<String> {
        let o = &self.options;
        let mut args = vec!["exec".to_string()];
        if let Some(id) = resume_id.filter(|id| !id.is_empty()) {
            args.extend(["resume".to_string(), id.to_string()]);
        }
        args.extend(["--json".to_string(), "--skip-git-repo-check".to_string()]);
        if let Some(model) = spec.model.as_ref().filter(|m| !m.is_empty()) {
            args.extend(["--model".to_string(), model.clone()]);
        }
        // Config overrides rather than --sandbox/--cd: they are accepted by `exec` and `exec resume` alike.
        let sandbox = o.sandbox.unwrap_or(CodexSandbox::WorkspaceWrite);
        args.extend([
            "-c".to_string(),
            format!("sandbox_mode=\"{}\"", sandbox.as_str()),
        ]);
        if sandbox == CodexSandbox::WorkspaceWrite && o.network_access.unwrap_or(true) {
            args.extend([
                "-c".to_string(),
                "sandbox_workspace_write.network_access=true".to_string(),
            ]);
        }
        for over in &o.config_overrides {
            args.extend(["-c".to_string(), over.clone()]);
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
        let mut parser = CodexStreamParser::new(move || clock.now(), self.options.pricing);
        let via_arg = self.options.prompt_via == PromptVia::Arg;
        let mut final_args = args;
        if via_arg {
            final_args.extend(["--".to_string(), prompt.to_string()]);
        } else {
            final_args.push("-".to_string());
        }
        let mut env = spec.env.clone().unwrap_or_default();
        if let Some(key) = self
            .cli
            .base_env
            .get("OPENAI_API_KEY")
            .filter(|k| !k.is_empty())
            && self
                .cli
                .base_env
                .get("CODEX_API_KEY")
                .is_none_or(|k| k.is_empty())
        {
            env.insert("CODEX_API_KEY".to_string(), key.clone());
        }
        let run_spec = AgentSpec {
            env: Some(env),
            ..spec.clone()
        };
        let res = {
            let mut on_line = |line: &str| {
                for e in parser.parse_line(line) {
                    on_event(e);
                }
            };
            self.cli
                .run(
                    &final_args,
                    &run_spec,
                    (!via_arg).then(|| prompt.to_string()),
                    &mut on_line,
                    cancel,
                )
                .await
        };
        if let Some(id) = parser.thread_id.as_ref().filter(|id| !id.is_empty()) {
            session.id = id.clone();
        }

        let fail = |message: String| -> AgentError {
            on_event(
                AgentEvent::new(self.cli.clock.now(), AgentEventType::Error)
                    .with_text(message.clone())
                    .with_data(json!({"exitCode": res.code(), "stderr": res.stderr_tail})),
            );
            AgentError::Failed {
                message,
                exit_code: res.code(),
                stderr: Some(res.stderr_tail.clone()),
            }
        };

        if res.aborted {
            on_event(
                AgentEvent::new(self.cli.clock.now(), AgentEventType::Status)
                    .with_text("codex turn aborted"),
            );
            return Err(AgentError::Aborted {
                session: (!session.id.is_empty()).then(|| session.clone()),
            });
        }
        if let Some(err) = res.spawn_error() {
            return Err(fail(format!("cannot run '{}': {err}", self.cli.binary)));
        }
        if parser.turn.failed {
            return Err(AgentError::Failed {
                message: parser
                    .turn
                    .error
                    .clone()
                    .unwrap_or_else(|| "codex turn failed".to_string()),
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
                format!("codex exited with {how}{tail}")
            } else {
                "codex stream ended without turn.completed".to_string()
            };
            return Err(fail(message));
        }
        if is_start && session.id.is_empty() {
            return Err(fail("codex did not report a thread id".to_string()));
        }
        Ok(())
    }
}

#[async_trait]
impl AgentAdapter for CodexAdapter {
    fn kind(&self) -> AgentKind {
        AgentKind::Codex
    }

    async fn preflight(&self) -> Preflight {
        let mut problems = Vec::new();
        if let Some(p) = self.cli.probe_binary().await {
            problems.push(p);
        }
        let has = |k: &str| self.cli.base_env.get(k).is_some_and(|v| !v.is_empty());
        if !self.options.allow_subscription_auth && !has("OPENAI_API_KEY") && !has("CODEX_API_KEY")
        {
            problems.push("OPENAI_API_KEY is not set".to_string());
        }
        Preflight {
            ok: problems.is_empty(),
            problems,
        }
    }

    async fn start(
        &self,
        spec: &AgentSpec,
        on_event: EventCallback<'_>,
        cancel: &CancellationToken,
    ) -> Result<AgentSession, AgentError> {
        let mut session = AgentSession {
            id: String::new(),
            kind: AgentKind::Codex,
        };
        self.turn(
            &mut session,
            self.build_args(spec, None),
            spec,
            &spec.prompt,
            on_event,
            cancel,
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
        on_event: EventCallback<'_>,
        cancel: &CancellationToken,
    ) -> Result<(), AgentError> {
        let args = self.build_args(spec, Some(&session.id));
        self.turn(session, args, spec, message, on_event, cancel, false)
            .await
    }
}
