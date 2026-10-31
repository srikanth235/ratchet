//! Local agents: the "This machine" mode. The real Claude Code / Codex adapters, driven by the same runner loop as in the cloud, but
//! using the CLIs that are installed and signed in on this machine (see `detect`), each agent in its own git worktree.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::claude_code::{ClaudeCodeAdapter, ClaudeCodeOptions};
use crate::cli_agent::CliAgentOptions;
use crate::codex::{CodexAdapter, CodexOptions};
use crate::control::local::{AdapterFor, LocalRunnerControl, LocalRunnerControlOptions, WorkdirMode};
use crate::detect::{HarnessAuth, HarnessInfo, HarnessKind, local_agent_env};
use crate::domain::{AgentAdapter, AgentKind, Clock, RunnerConfig};
use crate::process::SharedSpawner;
use crate::runner_loop::LoopTuning;

/// Builds the adapter for `kind: mock` (a demo with scripted agents).
pub type MockFactory = Arc<dyn Fn(&RunnerConfig) -> Arc<dyn AgentAdapter> + Send + Sync>;

#[derive(Clone, Default)]
pub struct LocalAdapterOptions {
    /// Result of `detect_harnesses`.
    pub harnesses: Vec<HarnessInfo>,
    /// The login-shell PATH (`resolve_login_path().path`), so the CLIs find node, git and the user's tools.
    pub login_path: String,
    /// Environment to derive the agents' from. Default: the process environment. Credentials are stripped, see `local_agent_env`.
    pub base_env: Option<HashMap<String, String>>,
    pub spawner: Option<SharedSpawner>,
    pub clock: Option<Arc<dyn Clock>>,
    /// Overrides for the Claude Code adapter (everything but the binary, environment and subscription auth, unless set).
    pub claude: Option<ClaudeCodeOptions>,
    pub codex: Option<CodexOptions>,
    /// Without it a mock agent is an error.
    pub mock: Option<MockFactory>,
}

/// Why an agent of `kind` cannot run on this machine, or `None` when it can.
pub fn local_agent_problem(harnesses: &[HarnessInfo], kind: HarnessKind) -> Option<String> {
    let label = match kind {
        HarnessKind::ClaudeCode => "Claude Code",
        HarnessKind::Codex => "Codex",
    };
    let Some(info) = harnesses.iter().find(|h| h.kind == kind) else { return Some(format!("{label} was not probed on this machine")) };
    if !info.installed || info.path.is_none() {
        return Some(info.hint.clone());
    }
    if info.auth == HarnessAuth::SignedOut {
        return Some(info.hint.clone());
    }
    None
}

fn merge_cli(common: CliAgentOptions, over: &CliAgentOptions) -> CliAgentOptions {
    CliAgentOptions {
        binary: over.binary.clone().or(common.binary),
        env: over.env.clone().or(common.env),
        spawner: over.spawner.clone().or(common.spawner),
        clock: over.clock.clone().or(common.clock),
        extra_args: if over.extra_args.is_empty() { common.extra_args } else { over.extra_args.clone() },
        kill_grace: over.kill_grace.or(common.kill_grace),
    }
}

/// `adapter_for` for `LocalRunnerControl`. Each adapter runs the binary found by detection with the user's login: no API key is
/// passed unless the harness reported `api-key` auth and the key is set. Fails (so the launch fails with the hint) when the harness
/// is missing or signed out.
pub fn local_adapter_factory(opts: LocalAdapterOptions) -> impl Fn(&RunnerConfig) -> Result<Arc<dyn AgentAdapter>, String> + Send + Sync {
    let base_env = opts.base_env.clone().unwrap_or_else(|| std::env::vars().collect());
    move |config: &RunnerConfig| {
        let kind = match config.kind {
            AgentKind::Mock => {
                let Some(mock) = &opts.mock else { return Err("mock agents are not available in this runtime".to_string()) };
                return Ok(mock(config));
            }
            AgentKind::ClaudeCode => HarnessKind::ClaudeCode,
            AgentKind::Codex => HarnessKind::Codex,
        };
        if let Some(problem) = local_agent_problem(&opts.harnesses, kind) {
            return Err(format!("cannot run {} on this machine: {problem}", config.kind));
        }
        let info = opts.harnesses.iter().find(|h| h.kind == kind).expect("checked by local_agent_problem");
        let env = local_agent_env(kind, info.auth, &base_env, &opts.login_path);
        let common = CliAgentOptions {
            binary: info.path.clone(),
            env: Some(env),
            spawner: opts.spawner.clone(),
            clock: opts.clock.clone(),
            ..CliAgentOptions::default()
        };
        Ok(match kind {
            HarnessKind::ClaudeCode => {
                let over = opts.claude.clone().unwrap_or_default();
                Arc::new(ClaudeCodeAdapter::new(ClaudeCodeOptions { cli: merge_cli(common, &over.cli), allow_subscription_auth: true, ..over })) as Arc<dyn AgentAdapter>
            }
            HarnessKind::Codex => {
                let over = opts.codex.clone().unwrap_or_default();
                Arc::new(CodexAdapter::new(CodexOptions { cli: merge_cli(common, &over.cli), allow_subscription_auth: true, ..over })) as Arc<dyn AgentAdapter>
            }
        })
    }
}

#[derive(Clone, Default)]
pub struct LocalAgentControlOptions {
    pub adapters: LocalAdapterOptions,
    /// `<root_dir>/<optimizationId>/{clone,<agentId>}`.
    pub root_dir: std::path::PathBuf,
    /// Git credentials for the clone and the pushes (e.g. `git_auth_env(repo_url, token)`). Never given to the agent processes.
    pub git_env: Option<HashMap<String, String>>,
    pub loop_tuning: LoopTuning,
    pub stop_grace: Option<Duration>,
}

/// A `LocalRunnerControl` that runs the real adapters, one git worktree per agent.
pub fn create_local_agent_control(opts: LocalAgentControlOptions) -> LocalRunnerControl {
    let factory = local_adapter_factory(opts.adapters.clone());
    let adapter_for: AdapterFor = Arc::new(move |config, _handle| factory(config));
    let mut options = LocalRunnerControlOptions::new(opts.root_dir, adapter_for);
    options.workdirs = WorkdirMode::Worktree;
    options.clock = opts.adapters.clock.clone();
    options.git_env = opts.git_env;
    options.loop_tuning = opts.loop_tuning;
    options.stop_grace = opts.stop_grace;
    LocalRunnerControl::new(options)
}
