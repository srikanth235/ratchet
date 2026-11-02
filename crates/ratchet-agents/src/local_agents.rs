//! Local agents: the "This machine" mode. The real Claude Code / Codex adapters, driven by the same runner loop as in the cloud, but
//! using the CLIs that are installed and signed in on this machine (see `detect`), each agent in its own git worktree.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use crate::claude_code::{ClaudeCodeAdapter, ClaudeCodeOptions};
use crate::cli_agent::CliAgentOptions;
use crate::codex::{CodexAdapter, CodexOptions};
use crate::control::local::{
    AdapterFor, ChainFor, LocalRunnerControl, LocalRunnerControlOptions, McpDone, McpFor,
    WorkdirMode,
};
use crate::detect::{
    DetectOptions, HarnessAuth, HarnessInfo, HarnessKind, detect_harnesses, local_agent_env,
};
use crate::grok::{GrokAdapter, GrokOptions};
use crate::process::SharedSpawner;
use crate::runner_loop::LoopTuning;
use async_trait::async_trait;
use ratchet_core::enums::AgentKind;
use ratchet_core::ports::{
    AgentAdapter, AgentSession, AgentSpec, CancellationToken, Clock, EventSink, PortError,
    PortResult, Preflight, RunnerConfig,
};

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
    pub grok: Option<GrokOptions>,
    /// Without it a mock agent is an error.
    pub mock: Option<MockFactory>,
}

/// Why an agent of `kind` cannot run on this machine, or `None` when it can.
pub fn local_agent_problem(harnesses: &[HarnessInfo], kind: HarnessKind) -> Option<String> {
    let label = match kind {
        HarnessKind::ClaudeCode => "Claude Code",
        HarnessKind::Codex => "Codex",
        HarnessKind::Grok => "Grok",
    };
    let Some(info) = harnesses.iter().find(|h| h.kind == kind) else {
        return Some(format!("{label} was not probed on this machine"));
    };
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
        extra_args: if over.extra_args.is_empty() {
            common.extra_args
        } else {
            over.extra_args.clone()
        },
        kill_grace: over.kill_grace.or(common.kill_grace),
        disable_live_session: over.disable_live_session || common.disable_live_session,
    }
}

/// `adapter_for` for `LocalRunnerControl`. Each adapter runs the binary found by detection with the user's login: no API key is
/// passed unless the harness reported `api-key` auth and the key is set. Fails (so the launch fails with the hint) when the harness
/// is missing or signed out.
pub fn local_adapter_factory(
    opts: LocalAdapterOptions,
) -> impl Fn(&RunnerConfig) -> Result<Arc<dyn AgentAdapter>, String> + Send + Sync {
    let base_env = opts
        .base_env
        .clone()
        .unwrap_or_else(|| std::env::vars().collect());
    move |config: &RunnerConfig| {
        if config.kind == AgentKind::Mock {
            let Some(mock) = &opts.mock else {
                return Err("mock agents are not available in this runtime".to_string());
            };
            return Ok(mock(config));
        }
        build_local_adapter(&opts, &base_env, config.kind)
    }
}

/// The adapter of one agent kind (the user's own login), for callers that need an agent outside a runner loop, such as
/// the environment setup agent. `mock` stands for Claude Code, as it does for the setup agent.
pub fn local_adapter_for(
    opts: &LocalAdapterOptions,
    kind: AgentKind,
) -> Result<Arc<dyn AgentAdapter>, String> {
    let base_env = opts
        .base_env
        .clone()
        .unwrap_or_else(|| std::env::vars().collect());
    let kind = if kind == AgentKind::Mock {
        AgentKind::ClaudeCode
    } else {
        kind
    };
    build_local_adapter(opts, &base_env, kind)
}

fn build_local_adapter(
    opts: &LocalAdapterOptions,
    base_env: &HashMap<String, String>,
    agent_kind: AgentKind,
) -> Result<Arc<dyn AgentAdapter>, String> {
    let kind = match agent_kind {
        AgentKind::ClaudeCode => HarnessKind::ClaudeCode,
        AgentKind::Codex => HarnessKind::Codex,
        AgentKind::Grok => HarnessKind::Grok,
        AgentKind::Mock => return Err("mock agents are not available in this runtime".to_string()),
    };
    if let Some(problem) = local_agent_problem(&opts.harnesses, kind) {
        return Err(format!(
            "cannot run {agent_kind} on this machine: {problem}"
        ));
    }
    let info = opts
        .harnesses
        .iter()
        .find(|h| h.kind == kind)
        .expect("checked by local_agent_problem");
    let env = local_agent_env(kind, info.auth, base_env, &opts.login_path);
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
            Arc::new(ClaudeCodeAdapter::new(ClaudeCodeOptions {
                cli: merge_cli(common, &over.cli),
                allow_subscription_auth: true,
                ..over
            })) as Arc<dyn AgentAdapter>
        }
        HarnessKind::Codex => {
            let over = opts.codex.clone().unwrap_or_default();
            Arc::new(CodexAdapter::new(CodexOptions {
                cli: merge_cli(common, &over.cli),
                allow_subscription_auth: true,
                ..over
            })) as Arc<dyn AgentAdapter>
        }
        HarnessKind::Grok => {
            let over = opts.grok.clone().unwrap_or_default();
            Arc::new(GrokAdapter::new(GrokOptions {
                cli: merge_cli(common, &over.cli),
                allow_subscription_auth: true,
                ..over
            })) as Arc<dyn AgentAdapter>
        }
    })
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
    /// The Ratchet optimization tool set for every turn of an agent (the daemon issues a session token per agent).
    pub mcp_for: Option<McpFor>,
    /// Called when an agent's loop ends, to revoke what `mcp_for` issued.
    pub mcp_done: Option<McpDone>,
}

/// A harness an agent may fall back to. Whether the CLI is installed and signed in can change while the optimization runs (the
/// user signs in again), so `preflight` detects the machine afresh and the adapter is built from what it finds.
pub struct RedetectingAdapter {
    kind: AgentKind,
    opts: LocalAdapterOptions,
    inner: tokio::sync::Mutex<Option<Arc<dyn AgentAdapter>>>,
}

impl RedetectingAdapter {
    pub fn new(kind: AgentKind, opts: LocalAdapterOptions) -> Self {
        Self {
            kind,
            opts,
            inner: tokio::sync::Mutex::new(None),
        }
    }

    fn build(&self, opts: &LocalAdapterOptions) -> Result<Arc<dyn AgentAdapter>, String> {
        local_adapter_for(opts, self.kind)
    }

    async fn current(&self) -> PortResult<Arc<dyn AgentAdapter>> {
        let mut inner = self.inner.lock().await;
        if let Some(a) = inner.as_ref() {
            return Ok(a.clone());
        }
        let a = self.build(&self.opts).map_err(PortError::Failed)?;
        *inner = Some(a.clone());
        Ok(a)
    }
}

#[async_trait]
impl AgentAdapter for RedetectingAdapter {
    fn kind(&self) -> AgentKind {
        self.kind
    }

    async fn preflight(&self) -> PortResult<Preflight> {
        let mut opts = self.opts.clone();
        opts.harnesses = detect_harnesses(&DetectOptions {
            login_path: Some(opts.login_path.clone()).filter(|p| !p.is_empty()),
            ..DetectOptions::default()
        })
        .await;
        match self.build(&opts) {
            Ok(a) => {
                *self.inner.lock().await = Some(a.clone());
                a.preflight().await
            }
            Err(problem) => Ok(Preflight {
                ok: false,
                problems: vec![problem],
            }),
        }
    }

    async fn start(
        &self,
        spec: &AgentSpec,
        events: &EventSink<'_>,
        cancel: Option<&CancellationToken>,
    ) -> PortResult<AgentSession> {
        self.current().await?.start(spec, events, cancel).await
    }

    async fn resume(
        &self,
        session: &mut AgentSession,
        spec: &AgentSpec,
        message: &str,
        events: &EventSink<'_>,
        cancel: Option<&CancellationToken>,
    ) -> PortResult<()> {
        self.current()
            .await?
            .resume(session, spec, message, events, cancel)
            .await
    }
}

/// A `LocalRunnerControl` that runs the real adapters, one git worktree per agent.
pub fn create_local_agent_control(opts: LocalAgentControlOptions) -> LocalRunnerControl {
    let factory = local_adapter_factory(opts.adapters.clone());
    let adapter_for: AdapterFor = Arc::new(move |config, _handle| factory(config));
    let mut options = LocalRunnerControlOptions::new(opts.root_dir, adapter_for);
    let chain_opts = opts.adapters.clone();
    let chain_for: ChainFor = Arc::new(move |kind| {
        Arc::new(RedetectingAdapter::new(kind, chain_opts.clone())) as Arc<dyn AgentAdapter>
    });
    options.chain_for = Some(chain_for);
    options.mcp_for = opts.mcp_for;
    options.mcp_done = opts.mcp_done;
    options.workdirs = WorkdirMode::Worktree;
    options.clock = opts.adapters.clock.clone();
    options.git_env = opts.git_env;
    options.loop_tuning = opts.loop_tuning;
    options.stop_grace = opts.stop_grace;
    LocalRunnerControl::new(options)
}
