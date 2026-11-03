//! Codex adapter: spawns `codex exec --json` and maps its JSONL to `AgentEvent`s.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Map, Value, json};

use crate::classify::{FailureKind, FailureOnce, classify_text};
use crate::claude_code::PromptVia;
use crate::cli_agent::{CliAgent, CliAgentOptions, as_record, num, str_of, truncate};
use crate::codex_app_server::CodexLiveSession;
use crate::detect::HarnessKind;
use crate::harness::{Scratch, TurnRunner, open_with_fallback};
use crate::models::{ModelCatalog, ModelUse, TokenPrice, resolve_model};
use crate::process::LineSplitter;
use ratchet_core::enums::{AgentKind, Billing};
use ratchet_core::ports::{
    Access, AgentAdapter, AgentEvent, AgentEventType, AgentSession, AgentSpec, AgentUsage,
    CancellationToken, Capabilities, EventSink, Harness, LiveSession, McpServer, PortError,
    PortResult, Preflight, SessionSpec, SharedEventSink,
};

/// USD per million tokens, for estimating cost: `codex exec --json` reports tokens but no dollars.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CodexPricing {
    pub input_per_m: f64,
    pub cached_input_per_m: Option<f64>,
    pub output_per_m: f64,
}

impl CodexPricing {
    /// The catalog's list price for a Codex model (by id or alias).
    pub fn for_model(catalog: &ModelCatalog, model: &str) -> Option<Self> {
        catalog.price_for(HarnessKind::Codex, model).map(Self::from)
    }
}

impl From<TokenPrice> for CodexPricing {
    fn from(p: TokenPrice) -> Self {
        Self {
            input_per_m: p.input_per_m,
            cached_input_per_m: p.cached_input_per_m,
            output_per_m: p.output_per_m,
        }
    }
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
    failure: FailureOnce,
    /// Report reasoning items as `Thinking` events (off for optimization runs, where they are noise).
    pub emit_thinking: bool,
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
            failure: FailureOnce::default(),
            emit_thinking: false,
            now: Box::new(now),
            pricing,
        }
    }

    /// How the turn failed, when it was a rate limit or an expired login.
    pub fn failure(&self) -> Option<FailureKind> {
        self.failure.kind()
    }

    /// The classified event for an error message (once per turn). The reset time of a Codex `exec` limit is only in the prose,
    /// whose format varies; it is not guessed.
    fn classified(&mut self, at: i64, message: &str) -> Option<AgentEvent> {
        let kind = classify_text(message, at / 1000)?;
        self.failure.emit(at, kind, message)
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
                let mut events = vec![
                    AgentEvent::new(at, AgentEventType::Error)
                        .with_text(message.clone())
                        .with_data(json!({"fatal": true})),
                ];
                events.extend(self.classified(at, &message));
                events
            }
            "error" => {
                // Often a transient stream error (reconnecting); `turn.failed` follows when it is fatal.
                let message = str_of(obj, "message").unwrap_or("codex error").to_string();
                self.last_error = Some(message.clone());
                let mut events = vec![
                    AgentEvent::new(at, AgentEventType::Error)
                        .with_text(message.clone())
                        .with_data(json!({"fatal": false})),
                ];
                events.extend(self.classified(at, &message));
                events
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
            Some("reasoning") => match str_of(item, "text") {
                Some(text) if self.emit_thinking && !text.trim().is_empty() => {
                    vec![AgentEvent::new(at, AgentEventType::Thinking).with_text(text)]
                }
                _ => Vec::new(),
            },
            Some("todo_list") => Vec::new(),
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
            ..AgentUsage::default()
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
    /// Hand MCP servers and the system-prompt add-on over in a temp `CODEX_HOME` (generated `config.toml` and `AGENTS.md`,
    /// the login linked in) instead of `-c` flags. The fallback for when the `-c mcp_servers...` form is not accepted by a
    /// CLI version, and the way to keep server environment values out of the process list.
    pub config_overlay: bool,
    /// Where the user's Codex home is, for the login the overlay links in. Default: `CODEX_HOME`, else `~/.codex`.
    pub codex_home: Option<String>,
}

/// A TOML basic string.
fn toml_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

/// A TOML key: bare when it can be, else quoted.
fn toml_key(k: &str) -> String {
    if !k.is_empty()
        && k.chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
    {
        k.to_string()
    } else {
        toml_str(k)
    }
}

fn toml_array(items: &[String]) -> String {
    format!(
        "[{}]",
        items
            .iter()
            .map(|i| toml_str(i))
            .collect::<Vec<_>>()
            .join(", ")
    )
}

/// `-c` values (`key=value`, TOML) that declare `servers`.
pub fn codex_mcp_overrides(servers: &[McpServer]) -> Vec<String> {
    let mut out = Vec::new();
    for s in servers {
        let name = toml_key(&s.name);
        out.push(format!(
            "mcp_servers.{name}.command={}",
            toml_str(&s.command)
        ));
        out.push(format!("mcp_servers.{name}.args={}", toml_array(&s.args)));
        for (k, v) in &s.env {
            out.push(format!(
                "mcp_servers.{name}.env.{}={}",
                toml_key(k),
                toml_str(v)
            ));
        }
    }
    out
}

/// The `[mcp_servers.<name>]` tables of an overlay `config.toml`.
pub fn codex_mcp_toml(servers: &[McpServer]) -> String {
    let mut out = String::new();
    for s in servers {
        let name = toml_key(&s.name);
        out.push_str(&format!("[mcp_servers.{name}]\n"));
        out.push_str(&format!("command = {}\n", toml_str(&s.command)));
        out.push_str(&format!("args = {}\n", toml_array(&s.args)));
        if !s.env.is_empty() {
            out.push_str(&format!("[mcp_servers.{name}.env]\n"));
            for (k, v) in &s.env {
                out.push_str(&format!("{} = {}\n", toml_key(k), toml_str(v)));
            }
        }
        out.push('\n');
    }
    out
}

#[derive(Clone)]
pub struct CodexAdapter {
    pub(crate) cli: CliAgent,
    pub(crate) options: CodexOptions,
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
        self.build_session_args(&SessionSpec::from(spec), resume_id)
    }

    /// The `-c` values for `spec`: sandbox, the user's own overrides, and (unless `config_overlay`) MCP servers and the system
    /// prompt add-on (`developer_instructions`).
    pub fn config_values(&self, spec: &SessionSpec) -> Vec<String> {
        let o = &self.options;
        let sandbox = if spec.access == Access::ReadOnly {
            CodexSandbox::ReadOnly
        } else {
            o.sandbox.unwrap_or(CodexSandbox::WorkspaceWrite)
        };
        let mut out = vec![format!("sandbox_mode=\"{}\"", sandbox.as_str())];
        let network = match spec.access {
            Access::Edit { network } => network && o.network_access.unwrap_or(true),
            Access::ReadOnly => false,
        };
        if sandbox == CodexSandbox::WorkspaceWrite && network {
            out.push("sandbox_workspace_write.network_access=true".to_string());
        }
        out.extend(o.config_overrides.iter().cloned());
        if !o.config_overlay {
            out.extend(codex_mcp_overrides(&spec.mcp));
            if let Some(text) = spec.system_append.as_ref().filter(|t| !t.is_empty()) {
                out.push(format!("developer_instructions={}", toml_str(text)));
            }
        }
        out
    }

    /// What a turn costs per token: the configured price, else the catalog's list price for the session's model (the default
    /// optimization model when none was named).
    pub fn pricing_for(&self, spec: &SessionSpec) -> Option<CodexPricing> {
        if self.options.pricing.is_some() {
            return self.options.pricing;
        }
        let catalog = ModelCatalog::bundled();
        let model = spec
            .model
            .as_deref()
            .filter(|m| !m.is_empty())
            .or_else(|| {
                catalog
                    .default_for(HarnessKind::Codex, ModelUse::Optimization)
                    .map(|m| m.id.as_str())
            })?;
        CodexPricing::for_model(catalog, model)
    }

    /// Argument list of a one-shot turn for `spec` (prompt excluded): [`CodexAdapter::build_args`] plus access, MCP servers and
    /// the system prompt add-on.
    pub fn build_session_args(&self, spec: &SessionSpec, resume_id: Option<&str>) -> Vec<String> {
        let mut args = vec!["exec".to_string()];
        if let Some(id) = resume_id.filter(|id| !id.is_empty()) {
            args.extend(["resume".to_string(), id.to_string()]);
        }
        args.extend(["--json".to_string(), "--skip-git-repo-check".to_string()]);
        if let Some(model) = spec.model.as_ref().filter(|m| !m.is_empty()) {
            let model = &resolve_model(HarnessKind::Codex, model).id;
            args.extend(["--model".to_string(), model.clone()]);
        }
        // Config overrides rather than --sandbox/--cd: they are accepted by `exec` and `exec resume` alike.
        for value in self.config_values(spec) {
            args.extend(["-c".to_string(), value]);
        }
        args.extend(self.cli.extra_args.iter().cloned());
        args
    }

    /// Argument list of `codex app-server` for `spec`. The `-c` values go before the subcommand: they are the CLI's own.
    pub fn build_app_server_args(&self, spec: &SessionSpec) -> Vec<String> {
        let mut args = Vec::new();
        for value in self.config_values(spec) {
            args.extend(["-c".to_string(), value]);
        }
        args.push("app-server".to_string());
        args
    }

    fn source_home(&self) -> Option<PathBuf> {
        self.options
            .codex_home
            .clone()
            .or_else(|| self.cli.base_env.get("CODEX_HOME").cloned())
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                self.cli
                    .base_env
                    .get("HOME")
                    .filter(|h| !h.is_empty())
                    .map(|h| Path::new(h).join(".codex"))
            })
    }

    /// With `config_overlay`: a temp `CODEX_HOME` holding the generated `config.toml` (MCP servers only; the user's own are not
    /// loaded), `AGENTS.md` (the system prompt add-on) and the user's login linked in. Returns its path.
    pub fn write_overlay(
        &self,
        spec: &SessionSpec,
        scratch: &Scratch,
    ) -> PortResult<Option<PathBuf>> {
        if !self.options.config_overlay {
            return Ok(None);
        }
        scratch.write("codex-home/config.toml", &codex_mcp_toml(&spec.mcp))?;
        if let Some(text) = spec.system_append.as_ref().filter(|t| !t.is_empty()) {
            scratch.write("codex-home/AGENTS.md", text)?;
        }
        let home = scratch.path().join("codex-home");
        #[cfg(unix)]
        if let Some(auth) = self
            .source_home()
            .map(|h| h.join("auth.json"))
            .filter(|p| p.exists())
        {
            let _ = std::os::unix::fs::symlink(auth, home.join("auth.json"));
        }
        Ok(Some(home))
    }

    /// The environment additions of a run: the API key under both names and the overlay home.
    pub(crate) fn run_env(
        &self,
        spec: &SessionSpec,
        overlay: Option<&Path>,
    ) -> BTreeMap<String, String> {
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
        if let Some(home) = overlay {
            env.insert(
                "CODEX_HOME".to_string(),
                home.to_string_lossy().into_owned(),
            );
        }
        env
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
            kind: AgentKind::Codex,
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
        let scratch = Scratch::new("codex")?;
        let overlay = self.write_overlay(spec, &scratch)?;
        let args = self.build_session_args(spec, resume_id);
        let clock = self.cli.clock.clone();
        let mut parser = CodexStreamParser::new(move || clock.now(), self.pricing_for(spec));
        let via_arg = self.options.prompt_via == PromptVia::Arg;
        let mut final_args = args;
        if via_arg {
            final_args.extend(["--".to_string(), prompt.to_string()]);
        } else {
            final_args.push("-".to_string());
        }
        let env = self.run_env(spec, overlay.as_deref());
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
                    Some(&env),
                    (!via_arg).then(|| prompt.to_string()),
                    &mut on_line,
                    cancel,
                )
                .await
        };
        drop(scratch);
        if let Some(id) = parser.thread_id.as_ref().filter(|id| !id.is_empty()) {
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
                    .with_text("codex turn aborted"),
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
impl TurnRunner for CodexAdapter {
    fn kind(&self) -> AgentKind {
        AgentKind::Codex
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
impl AgentAdapter for CodexAdapter {
    fn kind(&self) -> AgentKind {
        AgentKind::Codex
    }

    fn billing(&self) -> Billing {
        self.cli.billing
    }

    async fn preflight(&self) -> PortResult<Preflight> {
        let mut problems = Vec::new();
        if let Some(p) = self.cli.probe_binary().await {
            problems.push(p);
        }
        let has = |k: &str| self.cli.base_env.get(k).is_some_and(|v| !v.is_empty());
        if !self.options.allow_subscription_auth && !has("OPENAI_API_KEY") && !has("CODEX_API_KEY")
        {
            problems.push("OPENAI_API_KEY is not set".to_string());
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
impl Harness for CodexAdapter {
    fn capabilities(&self) -> Capabilities {
        Capabilities {
            // deltas only come from the app-server; `exec --json` sends whole messages
            streaming: !self.cli.disable_live_session,
            live_session: !self.cli.disable_live_session,
            mcp: true,
            cost_in_usd: false,
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
            "codex",
            runner,
            spec,
            events.clone(),
            || async { CodexLiveSession::open(self, spec, events).await },
        )
        .await
    }
}
