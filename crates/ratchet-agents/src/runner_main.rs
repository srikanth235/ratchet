//! The body of the `ratchet-runner` binary: reads its configuration from the environment and the artifacts store, runs the runner
//! loop for one agent, streams events to the store and writes `done.json`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::artifacts::{ArtifactEventSink, ArtifactInbox, AwsCliArtifactStore, EventSinkOptions, FsArtifactStore, config_key, done_key, runner_prefix};
use crate::claude_code::{ClaudeCodeAdapter, ClaudeCodeOptions, ClaudePermissionMode};
use crate::clock::{AcceleratedClock, SystemClock};
use crate::cli_agent::CliAgentOptions;
use crate::codex::{CodexAdapter, CodexOptions};
use crate::context::to_fixed;
use crate::demo_scripts::{DemoVariant, load_demo_search_script, load_script};
use crate::domain::{AgentAdapter, AgentKind, ArtifactStore, Clock, RunnerConfig};
use crate::git::{SharedGit, exec_git};
use crate::process::{RunProcessOptions, SharedSpawner, TokioSpawner, run_process};
use crate::runner_loop::{LoopTuning, RunnerLoopOptions, RunnerLoopResult, RunnerStopReason, run_runner_loop};
use crate::scripted::{ScriptedAgent, ScriptedAgentOptions};
use crate::workdir::{PrepareWorkdirOptions, git_auth_env, prepare_workdir};

/// A misconfiguration or an environment failure that stops the runner before or around the loop (the bin exits with 2).
#[derive(Debug, thiserror::Error)]
#[error("{0}")]
pub struct RunnerMainError(pub String);

impl RunnerMainError {
    fn new(message: impl Into<String>) -> Self {
        Self(message.into())
    }
}

pub type AdapterFactory = Arc<dyn Fn(&RunnerConfig, &HashMap<String, String>) -> Arc<dyn AgentAdapter> + Send + Sync>;

#[derive(Default)]
pub struct RunnerMainOptions {
    pub env: HashMap<String, String>,
    pub spawner: Option<SharedSpawner>,
    pub git: Option<SharedGit>,
    /// Replaces the S3/filesystem store chosen from the environment (tests).
    pub store: Option<Arc<dyn ArtifactStore>>,
    pub clock: Option<Arc<dyn Clock>>,
    pub signal: Option<CancellationToken>,
    /// Replaces the adapter chosen from `config.kind` (tests).
    pub adapter_factory: Option<AdapterFactory>,
    pub log: Option<Arc<dyn Fn(&str) + Send + Sync>>,
}

/// Env passed through to the agent CLI; deliberately small. The git token never goes in.
///
/// The proxy variables matter when the sandbox routes runner egress through its proxy: without them the agent CLIs cannot reach the
/// model APIs.
pub const AGENT_ENV_PASSTHROUGH: [&str; 17] = [
    "PATH",
    "HOME",
    "USER",
    "LANG",
    "LC_ALL",
    "TERM",
    "TMPDIR",
    "SHELL",
    "AWS_REGION",
    "AWS_DEFAULT_REGION",
    "HTTPS_PROXY",
    "https_proxy",
    "HTTP_PROXY",
    "http_proxy",
    "NO_PROXY",
    "no_proxy",
    "NODE_USE_ENV_PROXY",
];

/// `NAME=ref,NAME2=ref2` of SSM parameter names, fetched with `aws ssm get-parameter --with-decryption`. Returned in the order given.
pub fn parse_secret_refs(spec: Option<&str>) -> Result<Vec<(String, String)>, RunnerMainError> {
    let mut refs: Vec<(String, String)> = Vec::new();
    for part in spec.unwrap_or("").split(',').map(str::trim).filter(|s| !s.is_empty()) {
        let eq = part.find('=').filter(|eq| *eq > 0);
        let Some(eq) = eq else {
            return Err(RunnerMainError::new(format!("bad RATCHET_SECRET_REFS entry '{part}' (want NAME=/ssm/parameter/name)")));
        };
        let (name, reference) = (part[..eq].to_string(), part[eq + 1..].to_string());
        match refs.iter_mut().find(|(n, _)| *n == name) {
            Some(slot) => slot.1 = reference,
            None => refs.push((name, reference)),
        }
    }
    Ok(refs)
}

async fn fetch_secret(spawner: &SharedSpawner, env: &HashMap<String, String>, name: &str, reference: &str) -> Result<String, RunnerMainError> {
    let mut out = String::new();
    let mut collect = |l: &str| out.push_str(l);
    let mut options = RunProcessOptions::new(std::env::current_dir().unwrap_or_else(|_| ".".into()), env.clone());
    options.on_stdout_line = Some(&mut collect);
    let args: Vec<String> =
        ["ssm", "get-parameter", "--with-decryption", "--name", reference, "--query", "Parameter.Value", "--output", "text"].map(String::from).to_vec();
    let res = run_process(spawner.as_ref(), "aws", &args, options).await;
    if res.code() != Some(0) {
        return Err(RunnerMainError::new(format!("cannot read secret {name} from SSM {reference}: {}", res.stderr_tail)));
    }
    Ok(out)
}

fn required(env: &HashMap<String, String>, name: &str) -> Result<String, RunnerMainError> {
    match env.get(name) {
        Some(v) if !v.is_empty() => Ok(v.clone()),
        _ => Err(RunnerMainError::new(format!("{name} is required"))),
    }
}

/// Check that the config JSON has every required field, then decode it.
pub fn validate_config(value: &Value) -> Result<RunnerConfig, RunnerMainError> {
    const REQUIRED: [&str; 11] =
        ["optimizationId", "agentId", "kind", "repoUrl", "branch", "baseRef", "config", "brief", "baselineValue", "budgetUsd", "deadlineAt"];
    let object = value.as_object();
    let missing: Vec<&str> = REQUIRED.iter().copied().filter(|k| object.is_none_or(|o| !o.contains_key(*k))).collect();
    if !missing.is_empty() {
        return Err(RunnerMainError::new(format!("runner config is missing: {}", missing.join(", "))));
    }
    serde_json::from_value(value.clone()).map_err(|e| RunnerMainError::new(format!("runner config is invalid: {e}")))
}

pub fn default_adapter(
    config: &RunnerConfig,
    env: &HashMap<String, String>,
    agent_env: &HashMap<String, String>,
    spawner: SharedSpawner,
    clock: Arc<dyn Clock>,
) -> Result<Arc<dyn AgentAdapter>, RunnerMainError> {
    let cli = |binary_var: &str| CliAgentOptions {
        env: Some(agent_env.clone()),
        spawner: Some(spawner.clone()),
        clock: Some(clock.clone()),
        binary: env.get(binary_var).filter(|b| !b.is_empty()).cloned(),
        ..CliAgentOptions::default()
    };
    Ok(match config.kind {
        AgentKind::ClaudeCode => Arc::new(ClaudeCodeAdapter::new(ClaudeCodeOptions {
            cli: cli("RATCHET_CLAUDE_BIN"),
            permission_mode: env.get("RATCHET_CLAUDE_PERMISSION_MODE").filter(|m| !m.is_empty()).map(|m| ClaudePermissionMode::parse(m)),
            ..ClaudeCodeOptions::default()
        })),
        AgentKind::Codex => Arc::new(CodexAdapter::new(CodexOptions { cli: cli("RATCHET_CODEX_BIN"), ..CodexOptions::default() })),
        AgentKind::Mock => {
            let which = env.get("RATCHET_MOCK_SCRIPT").map(String::as_str).unwrap_or("cc");
            let script = match DemoVariant::parse(which) {
                Some(variant) => load_demo_search_script(variant, None),
                None => load_script(std::path::Path::new(which), None),
            }
            .map_err(|e| RunnerMainError::new(e.to_string()))?;
            Arc::new(ScriptedAgent::with_options(script, ScriptedAgentOptions { clock: Some(clock), ..ScriptedAgentOptions::default() }))
        }
    })
}

fn number_env(env: &HashMap<String, String>, name: &str) -> Result<Option<f64>, RunnerMainError> {
    match env.get(name) {
        None => Ok(None),
        Some(v) => v.trim().parse::<f64>().map(Some).map_err(|_| RunnerMainError::new(format!("{name} must be a number, got '{v}'"))),
    }
}

/// The body of the `ratchet-runner` binary. Returns the process exit code (0 done, 1 failed or agent unavailable).
pub async fn runner_main(options: RunnerMainOptions) -> Result<i32, RunnerMainError> {
    let env = &options.env;
    let log: Arc<dyn Fn(&str) + Send + Sync> = options.log.clone().unwrap_or_else(|| Arc::new(|l: &str| eprintln!("{l}")));
    let spawner: SharedSpawner = options.spawner.clone().unwrap_or_else(|| Arc::new(TokioSpawner));
    let clock: Arc<dyn Clock> = match &options.clock {
        Some(c) => c.clone(),
        None => {
            let speed = number_env(env, "RATCHET_CLOCK_SPEED")?.unwrap_or(1.0);
            if speed != 1.0 {
                Arc::new(AcceleratedClock::try_new(speed).map_err(RunnerMainError::new)?)
            } else {
                Arc::new(SystemClock)
            }
        }
    };

    let optimization_id = required(env, "RATCHET_OPTIMIZATION_ID")?;
    let agent_id = required(env, "RATCHET_AGENT_ID")?;
    let prefix = runner_prefix(&optimization_id, &agent_id);

    let store: Arc<dyn ArtifactStore> = match &options.store {
        Some(s) => s.clone(),
        None => {
            if let Some(bucket) = env.get("RATCHET_ARTIFACTS_BUCKET").filter(|b| !b.is_empty()) {
                Arc::new(AwsCliArtifactStore::new(bucket.clone(), Some(spawner.clone()), Some(env.clone()), env.get("AWS_REGION").cloned()))
            } else if let Some(dir) = env.get("RATCHET_ARTIFACT_DIR").filter(|d| !d.is_empty()) {
                Arc::new(FsArtifactStore::new(dir))
            } else {
                return Err(RunnerMainError::new("set RATCHET_ARTIFACTS_BUCKET (S3) or RATCHET_ARTIFACT_DIR (local directory)"));
            }
        }
    };

    // secrets from SSM: model API keys and the repo token
    let mut secrets: Vec<(String, String)> = Vec::new();
    for (name, reference) in parse_secret_refs(env.get("RATCHET_SECRET_REFS").map(String::as_str))? {
        let value = fetch_secret(&spawner, env, &name, &reference).await?;
        secrets.push((name, value));
    }
    let secret = |name: &str| secrets.iter().find(|(n, _)| n == name).map(|(_, v)| v.clone());

    let raw_config: Option<String> = match env.get("RATCHET_RUNNER_CONFIG") {
        Some(c) => Some(c.clone()),
        None => match env.get("RATCHET_RUNNER_CONFIG_FILE") {
            Some(file) => Some(tokio::fs::read_to_string(file).await.map_err(|e| RunnerMainError::new(format!("cannot read {file}: {e}")))?),
            None => store.get(&config_key(&prefix)).await.map_err(|e| RunnerMainError::new(e.to_string()))?,
        },
    };
    let Some(raw_config) = raw_config.filter(|c| !c.is_empty()) else {
        return Err(RunnerMainError::new(format!(
            "no runner config: set RATCHET_RUNNER_CONFIG[_FILE] or put {} in the artifacts store",
            config_key(&prefix)
        )));
    };
    let parsed: Value = serde_json::from_str(&raw_config).map_err(|e| RunnerMainError::new(format!("runner config is not valid JSON: {e}")))?;
    let config = validate_config(&parsed)?;
    if config.optimization_id != optimization_id || config.agent_id != agent_id {
        return Err(RunnerMainError::new("runner config does not match RATCHET_OPTIMIZATION_ID / RATCHET_AGENT_ID"));
    }

    let git_env: HashMap<String, String> = match secret("GITHUB_TOKEN").filter(|t| !t.is_empty()) {
        Some(token) => git_auth_env(&config.repo_url, &token),
        None => HashMap::new(),
    };
    let mut agent_env: HashMap<String, String> = HashMap::new();
    for k in AGENT_ENV_PASSTHROUGH {
        if let Some(v) = env.get(k) {
            agent_env.insert(k.to_string(), v.clone());
        }
    }
    for (k, v) in &secrets {
        if k != "GITHUB_TOKEN" {
            agent_env.insert(k.clone(), v.clone());
        }
    }

    let workdir: PathBuf = match env.get("RATCHET_WORKDIR") {
        Some(w) => PathBuf::from(w),
        None => env
            .get("RATCHET_WORK_ROOT")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("ratchet")
            .join(&optimization_id)
            .join(&agent_id),
    };
    let git: SharedGit = options.git.clone().unwrap_or_else(exec_git);
    prepare_workdir(PrepareWorkdirOptions {
        repo_url: config.repo_url.clone(),
        branch: config.branch.clone(),
        base_ref: config.base_ref.clone(),
        dir: workdir.clone(),
        git: Some(git.clone()),
        env: Some(git_env.clone()),
    })
    .await
    .map_err(|e| RunnerMainError::new(e.to_string()))?;

    let adapter: Arc<dyn AgentAdapter> = match &options.adapter_factory {
        Some(factory) => factory(&config, &agent_env),
        None => default_adapter(&config, env, &agent_env, spawner.clone(), clock.clone())?,
    };
    let flush_ms = number_env(env, "RATCHET_EVENT_FLUSH_MS")?.unwrap_or(2000.0);
    let poll_ms = number_env(env, "RATCHET_INBOX_POLL_MS")?.unwrap_or(2000.0);
    let sink = ArtifactEventSink::new(store.clone(), prefix.clone(), EventSinkOptions { flush_interval: Some(Duration::from_millis(flush_ms as u64)), max_buffered: None });
    let inbox = ArtifactInbox::new(store.clone(), prefix.clone()).with_clock(clock.clone()).with_poll_interval(Duration::from_millis(poll_ms as u64));

    log(&format!("ratchet-runner {optimization_id}/{agent_id} kind={} workdir={}", config.kind, workdir.display()));
    let tuning = LoopTuning {
        git: Some(git),
        git_env: Some(git_env),
        best_ref: env.get("RATCHET_BEST_REF").filter(|b| !b.is_empty()).cloned(),
        model: env.get("RATCHET_MODEL").filter(|m| !m.is_empty()).cloned(),
        max_idle_turns: number_env(env, "RATCHET_MAX_IDLE_TURNS")?.map(|n| n as u32),
        ..LoopTuning::default()
    };
    let result: RunnerLoopResult = run_runner_loop(RunnerLoopOptions {
        config,
        adapter,
        workdir,
        inbox: Arc::new(inbox),
        emit: sink.emitter(),
        clock: clock.clone(),
        signal: options.signal.clone(),
        tuning,
    })
    .await;
    sink.close().await;

    let mut done = serde_json::to_value(&result).map_err(|e| RunnerMainError::new(e.to_string()))?;
    if let Some(obj) = done.as_object_mut() {
        obj.insert("finishedAt".to_string(), Value::from(clock.now()));
    }
    store.put(&done_key(&prefix), done.to_string().as_bytes()).await.map_err(|e| RunnerMainError::new(e.to_string()))?;
    log(&format!("ratchet-runner finished: {} ({} candidates, ${})", result.reason, result.candidates, to_fixed(result.spend_usd, 2)));
    Ok(if matches!(result.reason, RunnerStopReason::Failed | RunnerStopReason::AgentUnavailable) { 1 } else { 0 })
}
