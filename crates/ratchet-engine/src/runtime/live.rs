//! Live runtime: the real engine wired to real ports. Port of `packages/runtime/src/live.ts`.
//! NOT exercised against a real account yet — see docs/REAL_ENV_TESTING.md for the test order.
//!
//! - Sandbox: `TofuSandboxProvider` (bundled `tofu`, infra/aws)
//! - AWS: `AwsCloudCompute` (EC2, IAM, SSM), `AwsPriceBook`, `S3ArtifactStore`
//! - Git: `GitHubGitHost` (repo-scoped token)
//! - Bench: `RemoteBenchRunner` (pinned EC2 bench instance via SSM)
//! - Agents: where the optimization says. `local` (the default of [`create_hybrid_runtime`]): the real Claude Code / Codex adapters
//!   on this machine through `LocalRunnerControl`, the user's own CLI login, one git worktree per agent. `cloud`:
//!   `SsmRunnerControl` (headless Claude Code / Codex on agent runners, needs API keys). The evaluator is always the cloud sandbox.
//!
//! The runtime is built in stages so the sandbox can be applied from the app: with `sandbox: None` (nothing applied yet) only the
//! tofu-backed sandbox ports work; everything that needs the sandbox's outputs, the GitHub repo or the AMIs is an "unready" port
//! whose calls fail with a `not_configured` error that says what is missing. Apply the sandbox, then build the runtime again with
//! its outputs (from the ledger).
//!
//! With `auto_sandbox` (the product default, see [`create_hybrid_runtime`]) there is no such staging: the engine creates the sandbox
//! at the first run start and destroys it after the idle window, and the AWS-backed ports are late-bound: each call reads the
//! sandbox's outputs from the ledger, so they work as soon as the sandbox is applied and fail with `not_configured` before that.
//!
//! Differences from the TypeScript on purpose: construction is `async` (the AWS SDK loads its configuration asynchronously); the
//! late binding is a hand-written proxy per port trait instead of a JavaScript `Proxy`.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ratchet_agents::{
    DetectOptions, HarnessInfo, LocalAdapterOptions, LocalAgentControlOptions, LocalRunnerControl,
    LoopTuning, SsmRunnerControl, SsmRunnerControlOptions, create_local_agent_control,
    detect_harnesses, git_auth_env, resolve_login_path,
};
use ratchet_bench::{RemoteBenchRunner, RemoteBenchRunnerOptions};
use ratchet_core::config::RatchetConfig;
use ratchet_core::enums::{AgentsWhere, Mode, SandboxProviderKind, SandboxStatus};
use ratchet_core::ids::{AgentId, OptimizationId};
use ratchet_core::ports::{
    AgentIdentity, ArtifactStore, BaselineMetrics, BenchJob, BenchLaunchSpec, BenchOutcome,
    BenchRunner, CancellationToken, CandidateCommit, Clock, CloudCompute, CommitFilesInput,
    CommitRef, FileDiff, GitHost, InstanceInfo, PolledEvents, PortError, PortResult,
    PullRequestInput, PullRequestRef, RemoteCommand, RemoteCommandResult, RunnerConfig,
    RunnerControl, RunnerHandle, RunnerLaunchSpec, RunnerMessage, SandboxInput, SandboxOutputs,
    SecretRef,
};
use ratchet_github::{GitHubGitHost, GitHubGitHostOptions};
use ratchet_ledger::Ledger;
use ratchet_sandbox::sandbox::{
    ContractCheckOptions, ProcessRunner, RuntimeRoleLister, TofuSandboxProvider,
    TofuSandboxProviderOptions,
};
use ratchet_sandbox::{
    AwsClients, AwsCloudCompute, AwsCloudComputeOptions, AwsComputeConfig, AwsCostReconciler,
    AwsPriceBook, Ec2ImageLookup, ImageLookup, S3ArtifactStore, StaticPriceBook,
    region_for_optimization,
};

use super::overlay_settings;
use crate::engine::{Engine, EngineOptions};
use crate::errors::{EngineError, EngineResult};
use crate::ports::{EnginePorts, GitSource};
use crate::settings::{EngineSettings, SandboxSettings, SecretSource};

const GITHUB_NEEDED: &str = "set the GitHub repository and token (Settings)";
const NEED_SANDBOX: &str = "apply the sandbox first (Environments)";
const NEED_BENCH_AMI: &str = "set the bench AMI (Settings)";

#[derive(Clone)]
pub struct LiveOptions {
    pub ledger: Ledger,
    pub region: String,
    /// Outputs of the applied sandbox (from the ledger / `TofuSandboxProvider::outputs`): bench subnet, split bench/confirm
    /// profiles, proxy. `None` before the first apply: the runtime then only plans and applies.
    pub sandbox: Option<SandboxOutputs>,
    pub sandbox_id: String,
    /// `owner/name` of the repository to optimize. Without it (or the token) git, bench and runners are unready.
    pub github_repo: Option<String>,
    pub github_token: Option<String>,
    /// AMI override for agent runners and bench instances ("Advanced", development only). Normally left unset: the AMIs are found
    /// by tag by [`resolve_live_images`] / [`create_live_runtime_for`], so the user never types an AMI id.
    pub runner_image_ref: Option<String>,
    pub bench_image_ref: Option<String>,
    /// The app version the published AMIs are matched against (`ratchet:image_version`).
    pub app_version: Option<String>,
    /// The sandbox module is a custom one (`infra:` in ratchet.yaml): validate its outputs against infra/contract.md after apply.
    pub infra_override: bool,
    /// Directory of the base module drafted environments are built on (`infra/modules/sandbox`); default: next to `module_dir`.
    pub base_module_dir: Option<PathBuf>,
    /// Candidate setup run on the bench before the guardrails, e.g. `pnpm install --frozen-lockfile && pnpm build`. The bench has
    /// no network: when this is set, dependencies come from a snapshot of the lockfile at `evaluator.ref`.
    pub bench_setup: Option<String>,
    /// Pinned `tofu` binary and the infra/aws module directory.
    pub tofu_binary: PathBuf,
    pub module_dir: PathBuf,
    pub state_root: PathBuf,
    /// Model API keys etc., looked up by agent kind + secret name.
    pub secrets: Option<SecretSource>,
    /// Named AWS profile (static, SSO or assume-role) used by every AWS client and by tofu.
    pub aws_profile: Option<String>,
    /// Extra environment for tofu (merged over `AWS_PROFILE`).
    pub aws_env: BTreeMap<String, String>,
    /// Replace the process runner that spawns `tofu` (tests).
    pub tofu_runner: Option<Arc<dyn ProcessRunner>>,
    pub clock: Option<Arc<dyn Clock>>,
    pub settings: EngineSettings,
    /// Where agents run by default. `local` needs `local_agents`. Default: `local` when `local_agents` is given, else `cloud`.
    pub agents_where: Option<AgentsWhere>,
    /// Agents on this machine: the detected harnesses and the login-shell PATH. See [`create_hybrid_runtime`], which detects them.
    pub local_agents: Option<LocalAgentsConfig>,
    /// The engine creates the sandbox at the first run start and destroys it after `sandbox_idle_ms` without a run (default 30
    /// min); the AWS ports bind to it late. Default false here (staged apply from Environments), true in the hybrid runtime.
    pub auto_sandbox: Option<bool>,
    pub sandbox_idle_ms: Option<i64>,
}

impl LiveOptions {
    /// The required fields; everything else off / default.
    pub fn new(
        ledger: Ledger,
        region: impl Into<String>,
        sandbox_id: impl Into<String>,
        tofu_binary: impl Into<PathBuf>,
        module_dir: impl Into<PathBuf>,
        state_root: impl Into<PathBuf>,
    ) -> Self {
        Self {
            ledger,
            region: region.into(),
            sandbox: None,
            sandbox_id: sandbox_id.into(),
            github_repo: None,
            github_token: None,
            runner_image_ref: None,
            bench_image_ref: None,
            app_version: None,
            infra_override: false,
            base_module_dir: None,
            bench_setup: None,
            tofu_binary: tofu_binary.into(),
            module_dir: module_dir.into(),
            state_root: state_root.into(),
            secrets: None,
            aws_profile: None,
            aws_env: BTreeMap::new(),
            tofu_runner: None,
            clock: None,
            settings: EngineSettings::default(),
            agents_where: None,
            local_agents: None,
            auto_sandbox: None,
            sandbox_idle_ms: None,
        }
    }
}

/// Agents on this machine (`LocalAgentsConfig` in TypeScript): the detected harnesses and login PATH live in `adapters`, the rest
/// tunes the worktree root and the runner loop.
#[derive(Clone, Default)]
pub struct LocalAgentsConfig {
    pub adapters: LocalAdapterOptions,
    /// Default `<state_root>/local-agents`.
    pub root_dir: Option<PathBuf>,
    pub loop_tuning: LoopTuning,
    pub stop_grace: Option<Duration>,
}

pub struct LiveRuntime {
    pub engine: Engine,
    pub sandbox: Arc<TofuSandboxProvider>,
    /// The GitHub remote, or `None` while git is not configured.
    pub repo_url: Option<String>,
    /// What still blocks running an optimization (empty = fully wired). The sandbox ports work regardless.
    pub missing: Vec<String>,
    /// Where agents run unless the optimization says otherwise.
    pub agents_where: AgentsWhere,
    /// The local agents control (`None` when no `local_agents` were given).
    pub local_runners: Option<Arc<LocalRunnerControl>>,
    /// The environment drafter; `None` without local agents or a GitHub repository.
    pub environment: Option<Arc<dyn ratchet_env::EnvironmentDrafter>>,
    /// The harnesses the local agents were built from (empty when none).
    pub harnesses: Vec<HarnessInfo>,
}

impl LiveRuntime {
    pub async fn dispose(&self) {
        self.engine.close().await;
        // Local agents die with the app; stopping them here also removes their worktrees.
        if let Some(l) = &self.local_runners {
            l.stop_all().await;
        }
    }
}

// ---------------------------------------------------------------------------------------------------- unready / late-bound ports

fn not_ready(port: &str, needs: &[String]) -> PortError {
    PortError::Failed(format!(
        "{}: {port} is not ready: {}",
        crate::engine::NOT_CONFIGURED,
        needs.join("; ")
    ))
}

/// The AWS-backed ports, built from one sandbox's outputs.
struct AwsPorts {
    compute: Arc<AwsCloudCompute>,
    artifacts: Arc<S3ArtifactStore>,
    bench: Option<Arc<RemoteBenchRunner>>,
    runners: Arc<SsmRunnerControl>,
}

/// Resolves the AWS ports on every call: the sandbox may be created (or replaced) after the runtime was built.
struct AwsResolver {
    ledger: Ledger,
    fixed: Option<SandboxOutputs>,
    region: String,
    sandbox_id: String,
    dynamic: bool,
    git: Option<Arc<dyn GitHost>>,
    bench_image_ref: Option<String>,
    bench_setup: Option<String>,
    profile: Option<String>,
    clock: Arc<dyn Clock>,
    cached: tokio::sync::Mutex<Option<(String, Arc<AwsPorts>)>>,
}

impl AwsResolver {
    /// The outputs the AWS ports are built from: the ones given, else (auto sandbox) the applied AWS sandbox in the ledger.
    async fn outputs_now(&self) -> PortResult<Option<(SandboxOutputs, String, String)>> {
        if let Some(o) = &self.fixed {
            return Ok(Some((
                o.clone(),
                self.region.clone(),
                self.sandbox_id.clone(),
            )));
        }
        if !self.dynamic {
            return Ok(None);
        }
        let rows: Vec<_> = self
            .ledger
            .list_sandboxes()
            .await
            .map_err(|e| PortError::failed(e.message))?
            .into_iter()
            .filter(|r| {
                r.provider == SandboxProviderKind::Aws
                    && r.status == SandboxStatus::Applied
                    && r.outputs.is_some()
            })
            .collect(); // newest first
        let row = rows
            .iter()
            .find(|r| r.id.as_str() == self.sandbox_id)
            .or_else(|| rows.first());
        let Some(row) = row else { return Ok(None) };
        let out: SandboxOutputs = serde_json::from_value(row.outputs.clone().unwrap_or_default())
            .map_err(|e| {
            PortError::failed(format!("sandbox {} has unreadable outputs: {e}", row.id))
        })?;
        Ok(Some((out, row.region.clone(), row.id.to_string())))
    }

    async fn now(&self) -> PortResult<Option<Arc<AwsPorts>>> {
        let Some((out, region, sandbox_id)) = self.outputs_now().await? else {
            return Ok(None);
        };
        let key = format!(
            "{sandbox_id}\0{region}\0{}",
            serde_json::to_string(&out).unwrap_or_default()
        );
        let mut cached = self.cached.lock().await;
        if let Some((k, ports)) = cached.as_ref()
            && *k == key
        {
            return Ok(Some(ports.clone()));
        }
        let profile = self.profile.as_deref();
        // Derived from outputs so the private bench subnet, the confirm-only profile and the egress proxy are never dropped.
        let mut cfg = AwsComputeConfig::from_outputs(&out, &sandbox_id);
        cfg.region = region.clone();
        let clients = AwsClients::connect(&region, profile).await;
        let compute = Arc::new(
            AwsCloudCompute::new(
                cfg,
                clients,
                AwsCloudComputeOptions {
                    clock: self.clock.clone(),
                    ..AwsCloudComputeOptions::default()
                },
            )
            .map_err(|e| PortError::failed(e.to_string()))?,
        );
        let artifacts = Arc::new(
            S3ArtifactStore::connect(out.artifacts_bucket.clone(), &region, profile).await,
        );
        let bench = match (&self.git, &self.bench_image_ref) {
            (Some(git), Some(image)) => {
                let mut o = RemoteBenchRunnerOptions::new(
                    compute.clone(),
                    git.clone(),
                    self.clock.clone(),
                    image.clone(),
                );
                o.artifacts = Some(artifacts.clone());
                // Bench instances have no internet: inputs are staged as tarballs under bench-inputs/ in the artifacts bucket.
                o.staging_bucket = Some(out.artifacts_bucket.clone());
                o.setup = self.bench_setup.clone();
                Some(Arc::new(RemoteBenchRunner::new(o)))
            }
            _ => None,
        };
        let runners = Arc::new(SsmRunnerControl::new(SsmRunnerControlOptions {
            compute: compute.clone(),
            store: artifacts.clone(),
            runner_command: None,
            runner_env: [
                (
                    "RATCHET_ARTIFACTS_BUCKET".to_owned(),
                    out.artifacts_bucket.clone(),
                ),
                ("AWS_REGION".to_owned(), region.clone()),
            ]
            .into(),
            state_dir: None,
        }));
        let ports = Arc::new(AwsPorts {
            compute,
            artifacts,
            bench,
            runners,
        });
        *cached = Some((key, ports.clone()));
        Ok(Some(ports))
    }
}

/// A port whose target is resolved per call; `None` means "not configured yet".
struct Late {
    port: &'static str,
    needs: Vec<String>,
    resolver: Arc<AwsResolver>,
}

impl Late {
    async fn ports(&self) -> PortResult<Arc<AwsPorts>> {
        self.resolver
            .now()
            .await?
            .ok_or_else(|| not_ready(self.port, &self.needs))
    }
}

struct LateCompute(Late);

#[async_trait]
impl CloudCompute for LateCompute {
    async fn create_agent_identity(
        &self,
        optimization_id: &OptimizationId,
        agent_id: &AgentId,
    ) -> PortResult<AgentIdentity> {
        self.0
            .ports()
            .await?
            .compute
            .create_agent_identity(optimization_id, agent_id)
            .await
    }
    async fn revoke_agent_identity(&self, identity: &AgentIdentity) -> PortResult<()> {
        self.0
            .ports()
            .await?
            .compute
            .revoke_agent_identity(identity)
            .await
    }
    async fn delete_agent_identity(&self, identity: &AgentIdentity) -> PortResult<()> {
        self.0
            .ports()
            .await?
            .compute
            .delete_agent_identity(identity)
            .await
    }
    async fn put_secret(
        &self,
        optimization_id: &OptimizationId,
        agent_id: &AgentId,
        name: &str,
        value: &str,
    ) -> PortResult<SecretRef> {
        self.0
            .ports()
            .await?
            .compute
            .put_secret(optimization_id, agent_id, name, value)
            .await
    }
    async fn delete_secrets(
        &self,
        optimization_id: &OptimizationId,
        agent_id: &AgentId,
    ) -> PortResult<()> {
        self.0
            .ports()
            .await?
            .compute
            .delete_secrets(optimization_id, agent_id)
            .await
    }
    async fn launch_runner(&self, spec: &RunnerLaunchSpec) -> PortResult<InstanceInfo> {
        self.0.ports().await?.compute.launch_runner(spec).await
    }
    async fn launch_bench_instance(&self, spec: &BenchLaunchSpec) -> PortResult<InstanceInfo> {
        self.0
            .ports()
            .await?
            .compute
            .launch_bench_instance(spec)
            .await
    }
    async fn terminate(&self, instance_ids: &[String]) -> PortResult<()> {
        self.0.ports().await?.compute.terminate(instance_ids).await
    }
    async fn list_tagged(
        &self,
        optimization_id: Option<&OptimizationId>,
    ) -> PortResult<Vec<InstanceInfo>> {
        self.0
            .ports()
            .await?
            .compute
            .list_tagged(optimization_id)
            .await
    }
    async fn run_command(
        &self,
        instance_id: &str,
        command: &RemoteCommand,
    ) -> PortResult<RemoteCommandResult> {
        self.0
            .ports()
            .await?
            .compute
            .run_command(instance_id, command)
            .await
    }
}

struct LateArtifacts(Late);

#[async_trait]
impl ArtifactStore for LateArtifacts {
    async fn put(&self, key: &str, body: &[u8]) -> PortResult<()> {
        self.0.ports().await?.artifacts.put(key, body).await
    }
    async fn get(&self, key: &str) -> PortResult<Option<String>> {
        self.0.ports().await?.artifacts.get(key).await
    }
    async fn list(&self, prefix: &str) -> PortResult<Vec<String>> {
        self.0.ports().await?.artifacts.list(prefix).await
    }
}

struct LateRunners(Late);

#[async_trait]
impl RunnerControl for LateRunners {
    async fn start(&self, handle: &RunnerHandle, config: &RunnerConfig) -> PortResult<()> {
        self.0.ports().await?.runners.start(handle, config).await
    }
    async fn send(&self, handle: &RunnerHandle, message: &RunnerMessage) -> PortResult<()> {
        self.0.ports().await?.runners.send(handle, message).await
    }
    async fn poll(&self, handle: &RunnerHandle, cursor: Option<&str>) -> PortResult<PolledEvents> {
        self.0.ports().await?.runners.poll(handle, cursor).await
    }
    async fn stop(&self, handle: &RunnerHandle) -> PortResult<()> {
        self.0.ports().await?.runners.stop(handle).await
    }
    async fn release(&self, optimization_id: &OptimizationId) -> PortResult<()> {
        self.0.ports().await?.runners.release(optimization_id).await
    }
}

struct LateBench(Late);

#[async_trait]
impl BenchRunner for LateBench {
    async fn run(
        &self,
        job: &BenchJob,
        config: &RatchetConfig,
        baseline: Option<&BaselineMetrics>,
        cancel: Option<&CancellationToken>,
    ) -> PortResult<BenchOutcome> {
        let ports = self.0.ports().await?;
        let Some(bench) = &ports.bench else {
            return Err(not_ready(self.0.port, &self.0.needs));
        };
        bench.run(job, config, baseline, cancel).await
    }
}

/// Git before a repository and token are configured: every call says what to set.
struct UnreadyGit;

fn github_unready() -> PortError {
    not_ready("GitHub", &[GITHUB_NEEDED.to_owned()])
}

#[async_trait]
impl GitHost for UnreadyGit {
    async fn create_branch(&self, _from_ref: &str, _branch: &str) -> PortResult<()> {
        Err(github_unready())
    }
    async fn list_candidates(
        &self,
        _branch: &str,
        _since_sha: Option<&str>,
    ) -> PortResult<Vec<CandidateCommit>> {
        Err(github_unready())
    }
    async fn changed_files(&self, _base_ref: &str, _sha: &str) -> PortResult<Vec<String>> {
        Err(github_unready())
    }
    async fn diff(&self, _base_ref: &str, _sha: &str) -> PortResult<Vec<FileDiff>> {
        Err(github_unready())
    }
    async fn checkout(&self, _sha: &str, _dir: &std::path::Path) -> PortResult<()> {
        Err(github_unready())
    }
    async fn open_pull_request(&self, _input: &PullRequestInput) -> PortResult<PullRequestRef> {
        Err(github_unready())
    }
    async fn resolve_ref(&self, _git_ref: &str) -> PortResult<String> {
        Err(github_unready())
    }
    fn remote_url(&self) -> String {
        String::new()
    }
    async fn commit_files(&self, _input: &CommitFilesInput) -> PortResult<CommitRef> {
        Err(github_unready())
    }
}

/// Teardown guard: runtime agent roles must be gone before `tofu destroy`. No outputs = nothing was ever launched.
struct ComputeRoleLister(Arc<AwsResolver>);

#[async_trait]
impl RuntimeRoleLister for ComputeRoleLister {
    async fn list_runtime_agent_roles(
        &self,
        _input: &SandboxInput,
    ) -> ratchet_sandbox::Result<Vec<String>> {
        let Some(ports) = self
            .0
            .now()
            .await
            .map_err(|e| ratchet_sandbox::Error::other(e.to_string()))?
        else {
            return Ok(Vec::new());
        };
        Ok(ports
            .compute
            .list_tagged(None)
            .await
            .map_err(|e| ratchet_sandbox::Error::other(e.to_string()))?
            .into_iter()
            .filter(|i| {
                i.role == ratchet_core::enums::InstanceRole::AgentRunner
                    && i.state != ratchet_core::ports::InstanceState::Terminated
            })
            .map(|i| i.id)
            .collect())
    }
}

// ---------------------------------------------------------------------------------------------------- createLiveRuntime

pub async fn create_live_runtime(opts: LiveOptions) -> EngineResult<LiveRuntime> {
    let clock: Arc<dyn Clock> = opts.clock.clone().unwrap_or_else(super::system_clock);
    let profile = opts.aws_profile.clone().filter(|p| !p.is_empty());
    let dynamic = opts.auto_sandbox == Some(true);
    let agents_where = opts.agents_where.unwrap_or(if opts.local_agents.is_some() {
        AgentsWhere::Local
    } else {
        AgentsWhere::Cloud
    });
    if agents_where == AgentsWhere::Local && opts.local_agents.is_none() {
        return Err(EngineError::invalid_input(
            "agentsWhere is local but no localAgents (detected harnesses) were given",
        ));
    }
    let mut missing: Vec<String> = Vec::new();
    if opts.sandbox.is_none() && !dynamic {
        missing.push(NEED_SANDBOX.to_owned());
    }
    let have_git = opts.github_repo.as_deref().is_some_and(|s| !s.is_empty())
        && opts.github_token.as_deref().is_some_and(|s| !s.is_empty());
    if !have_git {
        missing.push(GITHUB_NEEDED.to_owned());
    }
    if agents_where == AgentsWhere::Cloud && opts.runner_image_ref.is_none() {
        missing.push("set the runner AMI (Settings)".to_owned());
    }
    let bench_image = opts.bench_image_ref.clone().filter(|s| !s.is_empty());
    if bench_image.is_none() {
        missing.push(NEED_BENCH_AMI.to_owned());
    }
    if agents_where == AgentsWhere::Local
        && let Some(la) = &opts.local_agents
        && !la
            .adapters
            .harnesses
            .iter()
            .any(|h| h.installed && h.auth != ratchet_agents::HarnessAuth::SignedOut)
    {
        missing.push(
            "install and sign in to Claude Code or Codex on this machine (Settings > Agents)"
                .to_owned(),
        );
    }

    let github: Option<Arc<GitHubGitHost>> = if have_git {
        let mut o = GitHubGitHostOptions::new(
            opts.github_repo.clone().unwrap_or_default(),
            opts.github_token.clone().unwrap_or_default(),
        );
        o.cache_dir = Some(opts.state_root.join("git-cache"));
        Some(Arc::new(
            GitHubGitHost::new(o).map_err(|e| EngineError::invalid_input(e.to_string()))?,
        ))
    } else {
        None
    };
    let git: Arc<dyn GitHost> = match &github {
        Some(g) => g.clone(),
        None => Arc::new(UnreadyGit),
    };

    let resolver = Arc::new(AwsResolver {
        ledger: opts.ledger.clone(),
        fixed: opts.sandbox.clone(),
        region: opts.region.clone(),
        sandbox_id: opts.sandbox_id.clone(),
        dynamic,
        git: github.clone().map(|g| g as Arc<dyn GitHost>),
        bench_image_ref: bench_image.clone(),
        bench_setup: opts.bench_setup.clone().filter(|s| !s.is_empty()),
        profile: profile.clone(),
        clock: clock.clone(),
        cached: tokio::sync::Mutex::new(None),
    });
    // A contract piece missing from the given outputs fails here, as it always did.
    if !dynamic && opts.sandbox.is_some() {
        resolver
            .now()
            .await
            .map_err(|e| EngineError::invalid_input(e.to_string()))?;
    }

    let late = |port: &'static str, needs: Vec<String>| Late {
        port,
        needs,
        resolver: resolver.clone(),
    };
    let need_sandbox = vec![NEED_SANDBOX.to_owned()];
    let mut bench_needs: Vec<String> = Vec::new();
    if opts.sandbox.is_none() && !dynamic {
        bench_needs.push(NEED_SANDBOX.to_owned());
    }
    if !have_git {
        bench_needs.push(GITHUB_NEEDED.to_owned());
    }
    if bench_image.is_none() {
        bench_needs.push(NEED_BENCH_AMI.to_owned());
    }
    if bench_needs.is_empty() {
        bench_needs = need_sandbox.clone();
    }
    let compute: Arc<dyn CloudCompute> =
        Arc::new(LateCompute(late("AWS compute", need_sandbox.clone())));
    let artifacts: Arc<dyn ArtifactStore> =
        Arc::new(LateArtifacts(late("artifact store", need_sandbox.clone())));
    let bench: Arc<dyn BenchRunner> = Arc::new(LateBench(late("bench runner", bench_needs)));
    let runners: Arc<dyn RunnerControl> =
        Arc::new(LateRunners(late("agent runners", need_sandbox)));

    let mut sandbox_opts = TofuSandboxProviderOptions::new(
        opts.tofu_binary.clone(),
        opts.module_dir.clone(),
        opts.state_root.clone(),
        Arc::new(ComputeRoleLister(resolver.clone())),
    );
    if let Some(r) = &opts.tofu_runner {
        sandbox_opts.runner = r.clone();
    }
    if let Some(p) = &profile {
        sandbox_opts.env.insert("AWS_PROFILE".into(), p.clone());
    }
    sandbox_opts.env.extend(opts.aws_env.clone());
    sandbox_opts.base_module_dir = opts.base_module_dir.clone();
    // The machines of a drafted environment launch from the same bench image the evaluator uses.
    sandbox_opts.machine_image_id = bench_image.clone();
    sandbox_opts.image_version = opts.app_version.clone();
    if opts.infra_override {
        sandbox_opts.validate_contract = Some(ContractCheckOptions::default());
    }
    let sandbox = Arc::new(TofuSandboxProvider::new(sandbox_opts));

    let prices = Arc::new(
        AwsPriceBook::connect_with(
            opts.region.clone(),
            profile.as_deref(),
            clock.clone(),
            Arc::new(StaticPriceBook::new([])),
        )
        .await,
    );

    // Agents on this machine: the real adapters, the user's login, a git worktree each. Git credentials stay with the control.
    let local_runners = opts.local_agents.as_ref().map(|la| {
        let mut adapters = la.adapters.clone();
        if adapters.clock.is_none() {
            adapters.clock = Some(clock.clone());
        }
        Arc::new(create_local_agent_control(LocalAgentControlOptions {
            adapters,
            root_dir: la
                .root_dir
                .clone()
                .unwrap_or_else(|| opts.state_root.join("local-agents")),
            git_env: github
                .as_ref()
                .map(|g| git_auth_env(&g.remote_url(), opts.github_token.as_deref().unwrap_or(""))),
            loop_tuning: la.loop_tuning.clone(),
            stop_grace: la.stop_grace,
        }))
    });

    // The setup agent that drafts environments: the user's own Claude Code / Codex on this machine, plus `tofu plan` in
    // their account. Needs local agents and a GitHub repository to commit to.
    let environment: Option<Arc<dyn ratchet_env::EnvironmentDrafter>> =
        match (&opts.local_agents, &github) {
            (Some(la), Some(g)) => {
                let mut adapters = la.adapters.clone();
                if adapters.clock.is_none() {
                    adapters.clock = Some(clock.clone());
                }
                let adapter_for: ratchet_env::AdapterFactory = Arc::new(move |kind| {
                    ratchet_agents::local_adapter_for(&adapters, kind).map_err(PortError::Failed)
                });
                let host: Arc<dyn GitHost> = g.clone();
                let mut o = ratchet_env::AgentEnvironmentDrafterOptions::new(
                    ratchet_env::fixed_git(host),
                    adapter_for,
                    clock.clone(),
                );
                o.prices = Some(prices.clone());
                o.default_region = Some(opts.region.clone());
                o.scratch_dir = Some(opts.state_root.join("scratch"));
                let mut co = ratchet_sandbox::TofuEnvironmentCheckerOptions::new(
                    opts.tofu_binary.clone(),
                    opts.module_dir.clone(),
                );
                co.base_module_dir = opts.base_module_dir.clone();
                co.work_root = Some(opts.state_root.join("scratch"));
                if let Some(r) = &opts.tofu_runner {
                    co.runner = r.clone();
                }
                if let Some(p) = &profile {
                    co.env.insert("AWS_PROFILE".into(), p.clone());
                }
                co.env.extend(opts.aws_env.clone());
                co.machine_image_id = bench_image.clone();
                co.image_version = opts.app_version.clone();
                o.checker = Some(Arc::new(ratchet_sandbox::TofuEnvironmentChecker::new(co)));
                Some(Arc::new(ratchet_env::AgentEnvironmentDrafter::new(o)))
            }
            _ => None,
        };

    let costs = Arc::new(AwsCostReconciler::connect(profile.as_deref()).await);
    let mut base = EngineSettings {
        secrets: opts.secrets.clone(),
        runner_image_ref: opts.runner_image_ref.clone(),
        scratch_dir: Some(opts.state_root.join("scratch")),
        agents_where: Some(agents_where),
        sandbox: SandboxSettings {
            auto: Some(dynamic),
            default_region: Some(opts.region.clone()),
            idle_ms: opts.sandbox_idle_ms,
            ..SandboxSettings::default()
        },
        ..EngineSettings::default()
    };
    base = overlay_settings(base, opts.settings.clone());

    let engine = Engine::new(EngineOptions {
        ledger: opts.ledger.clone(),
        mode: Mode::Live,
        clock,
        ports: EnginePorts {
            sandbox: sandbox.clone(),
            compute,
            prices,
            artifacts,
            git: GitSource::One(git),
            runners,
            local_runners: local_runners.clone().map(|l| l as Arc<dyn RunnerControl>),
            bench,
            costs: Some(costs),
            environment: environment.clone(),
        },
        settings: base,
    })
    .await;

    Ok(LiveRuntime {
        engine,
        sandbox,
        repo_url: github.as_ref().map(|g| g.remote_url()),
        missing,
        agents_where,
        local_runners,
        environment,
        harnesses: opts
            .local_agents
            .as_ref()
            .map(|l| l.adapters.harnesses.clone())
            .unwrap_or_default(),
    })
}

// ---------------------------------------------------------------------------------------------------- hybrid

pub struct HybridOptions {
    /// Everything of a live runtime. `local_agents` and `auto_sandbox` are filled in by [`create_hybrid_runtime`];
    /// `agents_where` defaults to `local`.
    pub live: LiveOptions,
    /// Harnesses already detected (the app caches them); otherwise `detect_harnesses` runs.
    pub harnesses: Option<Vec<HarnessInfo>>,
    /// The login-shell PATH already resolved; otherwise `resolve_login_path` runs.
    pub login_path: Option<String>,
    /// Injection for the detection probes (tests).
    pub detect: DetectOptions,
    /// Rest of the local agents configuration (worktree root, base env, adapter options, loop tuning). `harnesses` and
    /// `login_path` of `adapters` are overwritten.
    pub local_agents: LocalAgentsConfig,
}

impl HybridOptions {
    pub fn new(live: LiveOptions) -> Self {
        Self {
            live,
            harnesses: None,
            login_path: None,
            detect: DetectOptions::default(),
            local_agents: LocalAgentsConfig::default(),
        }
    }
}

/// The product runtime: agents on this machine by default (`agents_where: cloud` for unattended runs), the evaluator in the cloud
/// sandbox (`RemoteBenchRunner` on AWS compute), and the sandbox itself managed by the engine. Probes the machine for Claude Code
/// and Codex unless the caller already has the result.
pub async fn create_hybrid_runtime(opts: HybridOptions) -> EngineResult<LiveRuntime> {
    let HybridOptions {
        mut live,
        harnesses,
        login_path,
        detect,
        mut local_agents,
    } = opts;
    let login_path = match login_path {
        Some(p) => p,
        None => resolve_login_path(&detect).await.path,
    };
    let harnesses = match harnesses {
        Some(h) => h,
        None => {
            detect_harnesses(&DetectOptions {
                login_path: Some(login_path.clone()),
                ..detect
            })
            .await
        }
    };
    local_agents.adapters.harnesses = harnesses;
    local_agents.adapters.login_path = login_path;
    live.agents_where = Some(live.agents_where.unwrap_or(AgentsWhere::Local));
    live.auto_sandbox = Some(live.auto_sandbox.unwrap_or(true));
    live.local_agents = Some(local_agents);
    create_live_runtime(live).await
}

// ---------------------------------------------------------------------------------------------------- images / region

/// Fills `runner_image_ref` / `bench_image_ref` from the published AMIs (EC2 DescribeImages by tag and `app_version`) when they are
/// not given. An explicit ref always wins. Without `app_version` nothing is looked up (the refs stay as given, and `missing` says
/// so). Lookup errors are `ImageLookupError`s that say which image to build. `lookup` replaces the EC2 lookup (tests).
pub async fn resolve_live_images(
    opts: LiveOptions,
    lookup: Option<&dyn ImageLookup>,
) -> EngineResult<LiveOptions> {
    let Some(app_version) = opts.app_version.clone().filter(|v| !v.is_empty()) else {
        return Ok(opts);
    };
    if opts.runner_image_ref.is_some() && opts.bench_image_ref.is_some() {
        return Ok(opts);
    }
    use ratchet_sandbox::images::RatchetImageKind;
    let need: Vec<RatchetImageKind> = [
        (RatchetImageKind::Runner, opts.runner_image_ref.is_none()),
        (RatchetImageKind::Bench, opts.bench_image_ref.is_none()),
    ]
    .into_iter()
    .filter_map(|(k, missing)| missing.then_some(k))
    .collect();
    let mut args = ratchet_sandbox::ResolveImageArgs::new(app_version, opts.region.clone());
    args.need = Some(need);
    let resolved = match lookup {
        Some(l) => ratchet_sandbox::resolve_image_refs(l, &args).await,
        None => {
            let l = Ec2ImageLookup::connect(&opts.region, opts.aws_profile.as_deref()).await;
            ratchet_sandbox::resolve_image_refs(&l, &args).await
        }
    }
    .map_err(|e| EngineError::invalid_input(e.to_string()))?;
    let mut out = opts;
    out.runner_image_ref = out.runner_image_ref.or(resolved.runner);
    out.bench_image_ref = out.bench_image_ref.or(resolved.bench);
    Ok(out)
}

/// The live runtime for ONE optimization: the region comes from the repo (`environment.region` in ratchet.yaml, else the settings'
/// region, else us-east-1) and the AMIs are found by tag in that region. `config` is the parsed ratchet.yaml as JSON (read
/// defensively).
pub async fn create_live_runtime_for(
    mut opts: LiveOptions,
    config: &serde_json::Value,
    lookup: Option<&dyn ImageLookup>,
) -> EngineResult<LiveRuntime> {
    let fallback = Some(opts.region.as_str()).filter(|r| !r.is_empty());
    opts.region = region_for_optimization(config, fallback)
        .map_err(|e| EngineError::invalid_input(e.to_string()))?;
    // Only look images up once a sandbox exists in that region: before the first apply there is nothing to launch.
    let resolved = if opts.sandbox.is_some() {
        resolve_live_images(opts, lookup).await?
    } else {
        opts
    };
    create_live_runtime(resolved).await
}
