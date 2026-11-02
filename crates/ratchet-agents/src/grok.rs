//! Grok adapter: spawns `grok -p - --output-format streaming-json` and maps its JSONL to `AgentEvent`s.

use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::classify::{FailureKind, FailureOnce, classify_text};
use crate::cli_agent::{CliAgent, CliAgentOptions, as_record, num, str_of, truncate};
use crate::detect::HarnessKind;
use crate::grok_acp::GrokLiveSession;
use crate::harness::{Scratch, TurnRunner, open_with_fallback};
use crate::models::resolve_model;
use crate::process::LineSplitter;
use ratchet_core::enums::AgentKind;
use ratchet_core::ports::{
    Access, AgentAdapter, AgentEvent, AgentEventType, AgentSession, AgentSpec, AgentUsage,
    CancellationToken, Capabilities, EventSink, Harness, LiveSession, McpServer, PortError,
    PortResult, Preflight, SessionSpec, SharedEventSink,
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
    failure: FailureOnce,
    /// Report each `text` / `thought` line as a `Delta` / `Thinking` event too (chat); optimization runs only want the joined
    /// `Message`. Default off.
    pub emit_deltas: bool,
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
            failure: FailureOnce::default(),
            emit_deltas: false,
            now: Box::new(now),
        }
    }

    /// How the turn failed, when it was a rate limit or an expired login.
    pub fn failure(&self) -> Option<FailureKind> {
        self.failure.kind()
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
            let mut events = Vec::new();
            if let Some(delta) = str_of(obj, "data") {
                self.text.push_str(delta);
                if self.emit_deltas && !delta.is_empty() {
                    events.push(AgentEvent::new(at, AgentEventType::Delta).with_text(delta));
                }
            }
            return events;
        }
        // Everything else ends the paragraph being streamed.
        let mut events = self.flush_text();
        match kind {
            "thought" => {
                if self.emit_deltas
                    && let Some(t) = str_of(obj, "data").filter(|t| !t.is_empty())
                {
                    events.push(AgentEvent::new(at, AgentEventType::Thinking).with_text(t));
                }
            }
            // The slash-command list and the per-step usage carry nothing Ratchet reports.
            "available_commands" | "usage" => {}
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
                        .with_text(message.clone())
                        .with_data(json!({"fatal": true})),
                );
                // Grok documents no rate-limit shape: the message text is all there is, and no reset time.
                if let Some(kind) = classify_text(&message, at / 1000) {
                    events.extend(self.failure.emit(at, kind, &message));
                }
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
    /// Set as `XAI_API_KEY` (and `GROK_CODE_XAI_API_KEY`) for the child (it takes precedence over a grok.com login).
    pub api_key: Option<String>,
    /// Skip the API-key preflight check: the CLI is signed in with a grok.com account (`grok login`) on this machine.
    pub allow_subscription_auth: bool,
    /// The tools a read-only session keeps (`--tools`). Default [`GROK_READ_ONLY_TOOLS`].
    pub read_only_tools: Option<Vec<String>>,
    /// The tools a read-only session must not have (`--disallowed-tools`; the denylist wins over the allowlist). Default
    /// [`GROK_WRITE_TOOLS`].
    pub write_tools: Option<Vec<String>>,
    /// Where the user's Grok home is, for the login the overlay links in. Default: `GROK_HOME`, else `~/.grok`.
    pub grok_home: Option<String>,
}

/// Tool names a read-only Grok session keeps. `read_file` is the name `available_commands` shows in the S0 fixtures; the rest
/// are best guesses until real-world testing (a wrong name only removes a tool).
pub const GROK_READ_ONLY_TOOLS: [&str; 4] = ["read_file", "list_dir", "grep", "glob"];

/// Tool names a read-only Grok session loses. `run_terminal_command` is in the fixtures; the rest are best guesses, and the
/// allowlist already excludes them.
pub const GROK_WRITE_TOOLS: [&str; 6] = [
    "run_terminal_command",
    "edit_file",
    "write_file",
    "apply_patch",
    "delete_file",
    "search_replace",
];

fn toml_str(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| "\"\"".to_string())
}

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

/// The overlay `config.toml`: the session's MCP servers only, and the user's own Claude / Cursor MCP sources switched off.
pub fn grok_config_toml(servers: &[McpServer]) -> String {
    let mut out = String::from(
        "# generated by Ratchet for one session\n[compat.claude]\nmcps = false\n\n[compat.cursor]\nmcps = false\n\n",
    );
    for s in servers {
        let name = toml_key(&s.name);
        out.push_str(&format!(
            "[mcp_servers.{name}]\ncommand = {}\n",
            toml_str(&s.command)
        ));
        out.push_str(&format!(
            "args = [{}]\n",
            s.args
                .iter()
                .map(|a| toml_str(a))
                .collect::<Vec<_>>()
                .join(", ")
        ));
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
pub struct GrokAdapter {
    pub(crate) cli: CliAgent,
    pub(crate) options: GrokOptions,
}

impl GrokAdapter {
    pub fn new(options: GrokOptions) -> Self {
        let mut cli = CliAgent::new("grok", options.cli.clone());
        if let Some(key) = &options.api_key {
            cli.base_env.insert("XAI_API_KEY".to_string(), key.clone());
        }
        Self { cli, options }
    }

    /// Argument list for one invocation, the prompt excluded (it is read from `prompt_file`, the `--prompt-file` of the headless
    /// mode; `-p -` does not read stdin). Exported for tests and docs.
    pub fn build_args(
        &self,
        spec: &AgentSpec,
        resume_id: Option<&str>,
        prompt_file: &Path,
    ) -> Vec<String> {
        self.build_session_args(&SessionSpec::from(spec), resume_id, prompt_file)
    }

    /// `--tools` / `--disallowed-tools` of a read-only session.
    pub(crate) fn read_only_tool_args(&self) -> Vec<String> {
        let o = &self.options;
        let keep = o
            .read_only_tools
            .clone()
            .unwrap_or_else(|| GROK_READ_ONLY_TOOLS.map(String::from).to_vec());
        let drop = o
            .write_tools
            .clone()
            .unwrap_or_else(|| GROK_WRITE_TOOLS.map(String::from).to_vec());
        vec![
            "--tools".to_string(),
            keep.join(","),
            "--disallowed-tools".to_string(),
            drop.join(","),
        ]
    }

    /// Argument list of a one-shot turn for `spec`: [`GrokAdapter::build_args`] plus access and the system prompt add-on.
    pub fn build_session_args(
        &self,
        spec: &SessionSpec,
        resume_id: Option<&str>,
        prompt_file: &Path,
    ) -> Vec<String> {
        let o = &self.options;
        let mut args: Vec<String> = vec![
            "--prompt-file".to_string(),
            prompt_file.to_string_lossy().into_owned(),
            "--output-format".to_string(),
            "streaming-json".to_string(),
        ];
        let read_only = spec.access == Access::ReadOnly;
        if !read_only {
            args.extend([
                "--permission-mode".to_string(),
                o.permission_mode
                    .clone()
                    .unwrap_or_else(|| "acceptEdits".to_string()),
            ]);
        }
        if let Some(id) = resume_id.filter(|id| !id.is_empty()) {
            args.extend(["--resume".to_string(), id.to_string()]);
        }
        if let Some(model) = spec.model.as_ref().filter(|m| !m.is_empty()) {
            let model = &resolve_model(HarnessKind::Grok, model).id;
            args.extend(["--model".to_string(), model.clone()]);
        }
        if read_only {
            args.extend(["--sandbox".to_string(), "read-only".to_string()]);
            args.extend(self.read_only_tool_args());
        } else if let Some(sandbox) = o.sandbox.as_ref().filter(|s| !s.is_empty()) {
            args.extend(["--sandbox".to_string(), sandbox.clone()]);
        }
        if let Some(turns) = spec.max_turns.filter(|t| *t > 0).or(o.max_turns) {
            args.extend(["--max-turns".to_string(), turns.to_string()]);
        }
        if let Some(text) = spec.system_append.as_ref().filter(|t| !t.is_empty()) {
            args.extend(["--rules".to_string(), text.clone()]);
        }
        args.extend(self.cli.extra_args.iter().cloned());
        args
    }

    fn source_home(&self) -> Option<PathBuf> {
        self.options
            .grok_home
            .clone()
            .or_else(|| self.cli.base_env.get("GROK_HOME").cloned())
            .filter(|h| !h.is_empty())
            .map(PathBuf::from)
            .or_else(|| {
                self.cli
                    .base_env
                    .get("HOME")
                    .filter(|h| !h.is_empty())
                    .map(|h| Path::new(h).join(".grok"))
            })
    }

    /// With MCP servers: a temp `GROK_HOME` holding the generated `config.toml` (the session's servers, the user's Claude and
    /// Cursor ones off) and the user's login linked in. One-shot `grok -p` has no MCP flag; the live (ACP) path passes the
    /// servers in `session/new` instead. Returns the home's path.
    pub fn write_overlay(
        &self,
        spec: &SessionSpec,
        scratch: &Scratch,
    ) -> PortResult<Option<PathBuf>> {
        if spec.mcp.is_empty() {
            return Ok(None);
        }
        scratch.write("grok-home/config.toml", &grok_config_toml(&spec.mcp))?;
        let home = scratch.path().join("grok-home");
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

    /// The environment additions of a run: the API key under both names the CLI may read, and the overlay home.
    pub(crate) fn run_env(
        &self,
        spec: &SessionSpec,
        overlay: Option<&Path>,
    ) -> BTreeMap<String, String> {
        let mut env = spec.env.clone().unwrap_or_default();
        let key = ["XAI_API_KEY", "GROK_CODE_XAI_API_KEY"]
            .iter()
            .find_map(|k| {
                env.get(*k)
                    .or_else(|| self.cli.base_env.get(*k))
                    .filter(|v| !v.is_empty())
                    .cloned()
            });
        if let Some(key) = key {
            env.entry("XAI_API_KEY".to_string())
                .or_insert_with(|| key.clone());
            env.entry("GROK_CODE_XAI_API_KEY".to_string())
                .or_insert(key);
        }
        if let Some(home) = overlay {
            env.insert("GROK_HOME".to_string(), home.to_string_lossy().into_owned());
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
            kind: AgentKind::Grok,
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
        let scratch = Scratch::new("grok")?;
        let prompt_file = scratch.write("prompt.txt", prompt)?;
        let overlay = self.write_overlay(spec, &scratch)?;
        let args = self.build_session_args(spec, resume_id, &prompt_file);
        let env = self.run_env(spec, overlay.as_deref());
        let clock = self.cli.clock.clone();
        let mut parser = GrokStreamParser::new(move || clock.now());
        parser.emit_deltas = false;
        let res = {
            let mut on_line = |line: &str| {
                for e in parser.parse_line(line) {
                    on_event(e);
                }
            };
            self.cli
                .run_in(&args, &spec.workdir, Some(&env), None, &mut on_line, cancel)
                .await
        };
        drop(scratch);
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
impl TurnRunner for GrokAdapter {
    fn kind(&self) -> AgentKind {
        AgentKind::Grok
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
        if !self.options.allow_subscription_auth
            && !has("XAI_API_KEY")
            && !has("GROK_CODE_XAI_API_KEY")
        {
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
impl Harness for GrokAdapter {
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
            "grok",
            runner,
            spec,
            events.clone(),
            || async { GrokLiveSession::open(self, spec, events).await },
        )
        .await
    }
}
