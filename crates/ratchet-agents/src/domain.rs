//! Domain types this crate shares with the rest of Ratchet.
//!
//! `ratchet-core` and `ratchet-sandbox` are being written in parallel, so the slices this crate needs are defined here, serde
//! compatible with the TypeScript JSON shapes (`packages/core/src/ports/index.ts`, `ledger/schema.ts`, `config/schema.ts`).
//! When those crates land, replace the definitions marked `TODO(core)` / `TODO(sandbox)` with re-exports.

use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;

// ---------------------------------------------------------------- ids and small enums (TODO(core))

pub type OptimizationId = String;
pub type AgentId = String;

/// TODO(core): `ledger::AgentKind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AgentKind {
    #[serde(rename = "claude-code")]
    ClaudeCode,
    #[serde(rename = "codex")]
    Codex,
    #[serde(rename = "mock")]
    Mock,
}

impl AgentKind {
    pub fn as_str(self) -> &'static str {
        match self {
            AgentKind::ClaudeCode => "claude-code",
            AgentKind::Codex => "codex",
            AgentKind::Mock => "mock",
        }
    }
}

impl fmt::Display for AgentKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// TODO(core): `ledger::CandidateVerdict`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CandidateVerdict {
    Pending,
    Verified,
    Rejected,
    FailedConfirm,
    Blocked,
}

impl CandidateVerdict {
    pub fn as_str(self) -> &'static str {
        match self {
            CandidateVerdict::Pending => "pending",
            CandidateVerdict::Verified => "verified",
            CandidateVerdict::Rejected => "rejected",
            CandidateVerdict::FailedConfirm => "failed_confirm",
            CandidateVerdict::Blocked => "blocked",
        }
    }
}

impl fmt::Display for CandidateVerdict {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------- clock (TODO(core))

/// Time source. `now` is epoch milliseconds. `sleep` returns (never errors) when `cancel` fires; callers re-check the token.
#[async_trait]
pub trait Clock: Send + Sync {
    fn now(&self) -> i64;
    async fn sleep(&self, duration: Duration, cancel: &CancellationToken);
}

// ---------------------------------------------------------------- agents (TODO(core))

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost_usd: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AgentEventType {
    Status,
    Message,
    Tool,
    Commit,
    Usage,
    Error,
    Verdict,
}

/// Conventions: `usage` is incremental (this event only). `status` events carry `data.state` in running | paused | done | failed.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AgentEvent {
    #[serde(default)]
    pub at: i64,
    #[serde(rename = "type")]
    pub kind: AgentEventType,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Map<String, Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<AgentUsage>,
}

impl AgentEvent {
    pub fn new(at: i64, kind: AgentEventType) -> Self {
        Self {
            at,
            kind,
            text: None,
            data: None,
            usage: None,
        }
    }

    pub fn with_text(mut self, text: impl Into<String>) -> Self {
        self.text = Some(text.into());
        self
    }

    /// Attach `data`. Top-level `null` entries are dropped, like `JSON.stringify` drops `undefined` fields.
    pub fn with_data(mut self, data: Value) -> Self {
        if let Value::Object(map) = data {
            self.data = Some(map.into_iter().filter(|(_, v)| !v.is_null()).collect());
        }
        self
    }

    pub fn with_usage(mut self, usage: AgentUsage) -> Self {
        self.usage = Some(usage);
        self
    }

    /// `data[key]` as a string.
    pub fn data_str(&self, key: &str) -> Option<&str> {
        self.data.as_ref()?.get(key)?.as_str()
    }
}

#[derive(Debug, Clone, Default)]
pub struct AgentSpec {
    pub workdir: PathBuf,
    pub prompt: String,
    pub model: Option<String>,
    pub env: Option<HashMap<String, String>>,
    pub max_turns: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentSession {
    pub id: String,
    pub kind: AgentKind,
}

/// Why an agent turn did not complete.
#[derive(Debug, Clone, thiserror::Error)]
pub enum AgentError {
    /// The agent CLI failed (non-zero exit, error result, missing session). The turn did not complete.
    #[error("{message}")]
    Failed {
        message: String,
        exit_code: Option<i32>,
        stderr: Option<String>,
    },
    /// The turn was cut short by the cancellation token (the CLI's process group was killed).
    #[error("agent turn aborted")]
    Aborted { session: Option<AgentSession> },
}

impl AgentError {
    pub fn failed(message: impl Into<String>) -> Self {
        AgentError::Failed {
            message: message.into(),
            exit_code: None,
            stderr: None,
        }
    }
    pub fn is_aborted(&self) -> bool {
        matches!(self, AgentError::Aborted { .. })
    }
}

pub type EventCallback<'a> = &'a (dyn Fn(AgentEvent) + Send + Sync);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Preflight {
    pub ok: bool,
    pub problems: Vec<String>,
}

/// Runs inside `ratchet-runner` on an agent runner (or locally in simulation). TODO(core): `ports::AgentAdapter`.
#[async_trait]
pub trait AgentAdapter: Send + Sync {
    fn kind(&self) -> AgentKind;
    async fn preflight(&self) -> Preflight;
    /// Run one agent turn to completion, streaming events.
    async fn start(
        &self,
        spec: &AgentSpec,
        on_event: EventCallback<'_>,
        cancel: &CancellationToken,
    ) -> Result<AgentSession, AgentError>;
    /// Continue the same session with a new message; resolves when the turn ends. The session id is updated if the CLI forks one.
    async fn resume(
        &self,
        session: &mut AgentSession,
        spec: &AgentSpec,
        message: &str,
        on_event: EventCallback<'_>,
        cancel: &CancellationToken,
    ) -> Result<(), AgentError>;
}

// ---------------------------------------------------------------- runner control (TODO(core))

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "lowercase",
    rename_all_fields = "camelCase"
)]
pub enum RunnerMessage {
    Verdict {
        sha: String,
        verdict: CandidateVerdict,
        reasons: Vec<String>,
        current_best: f64,
    },
    /// The shared ratchet moved (possibly by another agent): rebase onto `sha` from `branch`.
    Ratchet {
        sha: String,
        branch: String,
        value: f64,
    },
    User {
        text: String,
    },
    Pause,
    Resume,
    Stop,
    /// A message with a `type` this version does not know. Reported by the loop, never fatal.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Direction {
    Minimize,
    Maximize,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Objective {
    pub metric: String,
    pub direction: Direction,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_gain: Option<f64>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DepsMode {
    #[default]
    Auto,
    Pnpm,
    Npm,
    None,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evaluator {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    #[serde(default)]
    pub deps: DepsMode,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Comparator {
    #[serde(rename = "<")]
    Lt,
    #[serde(rename = "<=")]
    Le,
    #[serde(rename = ">")]
    Gt,
    #[serde(rename = ">=")]
    Ge,
    #[serde(rename = "==")]
    Eq,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Guardrail {
    Command {
        run: String,
    },
    Threshold {
        metric: String,
        op: Comparator,
        value: f64,
        relative: bool,
        raw: String,
    },
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Environment {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

/// The slice of the parsed `ratchet.yaml` the runner needs (TODO(core): `config::RatchetConfig`). Unknown fields are kept so the
/// config round-trips through `config.json` unchanged.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RatchetConfig {
    #[serde(default)]
    pub version: u32,
    pub objective: Objective,
    pub evaluator: Evaluator,
    #[serde(default)]
    pub guardrails: Vec<Guardrail>,
    #[serde(default, rename = "protected")]
    pub protected: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<Environment>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub infra: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerConfig {
    pub optimization_id: OptimizationId,
    pub agent_id: AgentId,
    pub kind: AgentKind,
    pub repo_url: String,
    pub branch: String,
    pub base_ref: String,
    pub config: RatchetConfig,
    /// RATCHET.md contents.
    pub brief: String,
    pub baseline_value: f64,
    /// The win threshold in force (a fraction): `min_gain`, or the one calibrated from the baseline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub min_gain: Option<f64>,
    pub budget_usd: f64,
    pub deadline_at: i64,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerHandle {
    pub optimization_id: OptimizationId,
    pub agent_id: AgentId,
    pub instance_id: String,
}

/// Failure of a runner-control operation (start, send, poll, stop).
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct ControlError(pub String);

impl ControlError {
    pub fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

/// Events since a cursor, and the next cursor.
#[derive(Debug, Clone, PartialEq)]
pub struct PollResult {
    pub events: Vec<AgentEvent>,
    pub cursor: String,
}

/// TODO(core): `ports::RunnerControl`. The engine's only way to reach a runner.
#[async_trait]
pub trait RunnerControl: Send + Sync {
    async fn start(&self, handle: &RunnerHandle, config: &RunnerConfig)
    -> Result<(), ControlError>;
    async fn send(&self, handle: &RunnerHandle, message: RunnerMessage)
    -> Result<(), ControlError>;
    /// Events since `cursor` (opaque); returns the next cursor.
    async fn poll(
        &self,
        handle: &RunnerHandle,
        cursor: Option<&str>,
    ) -> Result<PollResult, ControlError>;
    async fn stop(&self, handle: &RunnerHandle) -> Result<(), ControlError>;
    /// Free whatever the control holds on this machine for an optimization (local worktrees and clones).
    async fn release(&self, _optimization_id: &str) -> Result<(), ControlError> {
        Ok(())
    }
}

/// The inbox the runner loop reads engine messages from. TODO(core): `ports::RunnerInbox`.
#[async_trait]
pub trait RunnerInbox: Send + Sync {
    /// Resolves with the next message, or `None` when `cancel` fires.
    async fn next(&self, cancel: &CancellationToken) -> Option<RunnerMessage>;
}

// ---------------------------------------------------------------- artifact store and cloud compute (TODO(sandbox))

/// Failure of an artifact-store operation.
#[derive(Debug, Clone, thiserror::Error)]
#[error("{0}")]
pub struct StoreError(pub String);

/// TODO(sandbox): the S3-backed store. Keys are `/`-separated; `list` returns full keys under `prefix`, sorted.
#[async_trait]
pub trait ArtifactStore: Send + Sync {
    async fn put(&self, key: &str, body: &[u8]) -> Result<(), StoreError>;
    /// `Ok(None)` when the object does not exist.
    async fn get(&self, key: &str) -> Result<Option<String>, StoreError>;
    async fn list(&self, prefix: &str) -> Result<Vec<String>, StoreError>;
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteCommand {
    /// Shell script run on the instance (SSM RunShellScript).
    pub script: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub env: Option<HashMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_seconds: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RemoteCommandResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// TODO(sandbox): `CloudCompute`. Only the SSM command channel is used here.
#[async_trait]
pub trait CloudCompute: Send + Sync {
    async fn run_command(
        &self,
        instance_id: &str,
        command: RemoteCommand,
    ) -> Result<RemoteCommandResult, ControlError>;
}

// ---------------------------------------------------------------- protected set (TODO(core))

/// Lockfiles and install configuration, at any depth. Dependency changes are not allowed in v0.
pub const INSTALL_FILES: [&str; 11] = [
    "pnpm-lock.yaml",
    "package-lock.json",
    "npm-shrinkwrap.json",
    "yarn.lock",
    "bun.lock",
    "bun.lockb",
    ".npmrc",
    ".yarnrc",
    ".yarnrc.yml",
    "pnpm-workspace.yaml",
    ".pnpmfile.cjs",
];

const EVALUATOR_DIRS: [&str; 2] = ["bench", "eval"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProtectedSource {
    Ratchet,
    Evaluator,
    Environment,
    Install,
    User,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtectedEntry {
    pub glob: String,
    pub source: ProtectedSource,
}

/// `/` joined, no `.` segments, no leading `./`, no trailing `/` (the part of core's `normalizeRepoPath` this needs).
fn normalize_repo_path(path: &str) -> String {
    let unified = path.replace('\\', "/");
    let parts: Vec<&str> = unified
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    parts.join("/")
}

fn is_word(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

fn is_segment_char(c: char) -> bool {
    is_word(c) || matches!(c, '.' | '@' | '-')
}

/// `bench/` and `eval/` directories referenced by an evaluation command. For `node dist/bench/x.js` that is `dist/bench/**` and,
/// as the likely source of the built file, `bench/**`. Paths that escape the repo are ignored.
///
/// Equivalent of the TS regex `(?<![\w.@/])((?:\./)?(?:[\w.@-]+/)*?)(bench|eval)(?=/)` applied with `matchAll`.
pub fn evaluator_dirs_from_run(run: &str) -> Vec<String> {
    let chars: Vec<char> = run.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if i > 0 && (is_word(chars[i - 1]) || matches!(chars[i - 1], '.' | '@' | '/')) {
            i += 1;
            continue;
        }
        match match_dir_at(&chars, i) {
            Some((prefix, dir, end)) => {
                let prefix = normalize_repo_path(&prefix);
                i = end;
                if prefix.split('/').any(|s| s == "..") || prefix.starts_with("..") {
                    continue;
                }
                let full = if prefix.is_empty() {
                    dir.to_string()
                } else {
                    format!("{prefix}/{dir}")
                };
                out.push(format!("{full}/**"));
                if !prefix.is_empty() {
                    out.push(format!("{dir}/**"));
                }
            }
            None => i += 1,
        }
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|g| seen.insert(g.clone()));
    out
}

/// Try to match `(?:\./)?(?:[\w.@-]+/)*?(bench|eval)(?=/)` starting at `start`; returns (prefix text, dir, end of dir).
fn match_dir_at(chars: &[char], start: usize) -> Option<(String, &'static str, usize)> {
    let starts_with = |pos: usize, s: &str| {
        s.chars()
            .enumerate()
            .all(|(k, c)| chars.get(pos + k) == Some(&c))
    };
    let lead_options: Vec<usize> = if starts_with(start, "./") {
        vec![start + 2, start]
    } else {
        vec![start]
    };
    for lead in lead_options {
        let mut pos = lead;
        loop {
            for dir in EVALUATOR_DIRS {
                if starts_with(pos, dir) && chars.get(pos + dir.len()) == Some(&'/') {
                    let prefix: String = chars[start..pos].iter().collect();
                    return Some((prefix, dir, pos + dir.len()));
                }
            }
            // consume one more `[\w.@-]+/` segment (greedy within the segment, as the regex does)
            let mut end = pos;
            while end < chars.len() && is_segment_char(chars[end]) {
                end += 1;
            }
            if end == pos || chars.get(end) != Some(&'/') {
                break;
            }
            pos = end + 1;
        }
    }
    None
}

/// The effective protected set for a config. `template_protect` returns the globs an evaluator template protects (TODO(core): from
/// the template registry); pass `|_| vec![]` when templates are not available.
pub fn effective_protected_with(
    config: &RatchetConfig,
    template_protect: &dyn Fn(&str) -> Vec<String>,
) -> Vec<ProtectedEntry> {
    let mut entries: Vec<ProtectedEntry> = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut add = |glob: &str, source: ProtectedSource| {
        let g = glob.replace('\\', "/");
        let mut g = g.as_str();
        while let Some(rest) = g.strip_prefix("./") {
            g = rest;
        }
        let g = g.trim_start_matches('/');
        if g.is_empty() || !seen.insert(g.to_string()) {
            return;
        }
        entries.push(ProtectedEntry {
            glob: g.to_string(),
            source,
        });
    };

    add("ratchet.yaml", ProtectedSource::Ratchet);
    if let Some(template) = &config.evaluator.template {
        for g in template_protect(template) {
            add(&g, ProtectedSource::Evaluator);
        }
    }
    if let Some(run) = &config.evaluator.run {
        for g in evaluator_dirs_from_run(run) {
            add(&g, ProtectedSource::Evaluator);
        }
    }
    if let Some(image) = config.environment.as_ref().and_then(|e| e.image.as_deref()) {
        let image = normalize_repo_path(image);
        if !image.is_empty() {
            add(&image, ProtectedSource::Environment);
            if image.starts_with(".devcontainer/") {
                add(".devcontainer/**", ProtectedSource::Environment);
            }
        }
    }
    if let Some(infra) = &config.infra {
        add(&normalize_repo_path(infra), ProtectedSource::Environment);
    }
    for f in INSTALL_FILES {
        add(&format!("**/{f}"), ProtectedSource::Install);
    }
    for g in &config.protected {
        add(g, ProtectedSource::User);
    }
    entries
}

/// `effective_protected_with` without template globs.
pub fn effective_protected(config: &RatchetConfig) -> Vec<ProtectedEntry> {
    effective_protected_with(config, &|_| Vec::new())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluator_dirs() {
        assert_eq!(
            evaluator_dirs_from_run("node dist/bench/x.js"),
            vec!["dist/bench/**", "bench/**"]
        );
        assert_eq!(
            evaluator_dirs_from_run("k6 run bench/k6/search.js"),
            vec!["bench/**"]
        );
        assert_eq!(
            evaluator_dirs_from_run("./eval/run.sh && python eval/x.py"),
            vec!["eval/**"]
        );
        assert!(evaluator_dirs_from_run("node ../bench/x.js").is_empty());
        assert!(evaluator_dirs_from_run("echo benchmark/x").is_empty());
    }

    #[test]
    fn runner_message_wire_shape() {
        let m: RunnerMessage =
            serde_json::from_str(r#"{"type":"verdict","sha":"abc","verdict":"failed_confirm","reasons":["x"],"currentBest":1.5}"#).unwrap();
        assert_eq!(
            m,
            RunnerMessage::Verdict {
                sha: "abc".into(),
                verdict: CandidateVerdict::FailedConfirm,
                reasons: vec!["x".into()],
                current_best: 1.5
            }
        );
        assert_eq!(
            serde_json::to_string(&RunnerMessage::Stop).unwrap(),
            r#"{"type":"stop"}"#
        );
        assert_eq!(
            serde_json::from_str::<RunnerMessage>(r#"{"type":"from-the-future"}"#).unwrap(),
            RunnerMessage::Unknown
        );
    }

    #[test]
    fn event_wire_shape_matches_ts() {
        let e = AgentEvent::new(5, AgentEventType::Usage).with_usage(AgentUsage {
            input_tokens: 1,
            output_tokens: 2,
            cost_usd: None,
        });
        assert_eq!(
            serde_json::to_string(&e).unwrap(),
            r#"{"at":5,"type":"usage","usage":{"inputTokens":1,"outputTokens":2}}"#
        );
        // `at` is optional on read (older writers omit it)
        let e: AgentEvent = serde_json::from_str(r#"{"type":"status","text":"one"}"#).unwrap();
        assert_eq!((e.at, e.text.as_deref()), (0, Some("one")));
    }
}
