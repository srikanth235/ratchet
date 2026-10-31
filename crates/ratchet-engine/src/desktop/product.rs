//! The composition point of the app: routes every command to the right runtime. Port of `packages/desktop/src/main/engine.ts`
//! (`ProductEngine`).
//!
//! - Real runs (a GitHub repo): one hybrid runtime per repository ([`create_hybrid_runtime`]). Agents run on this machine by
//!   default (the user's own Claude Code / Codex login), or on cloud runners; the evaluator always runs in the cloud sandbox,
//!   which the engine creates at the first run and removes after the idle window.
//! - Demos: the same real [`Engine`] on mock AWS + local git + local bench + scripted agents ([`create_simulation_runtime`]),
//!   one runtime per demo project, created on first use. No setup, no cloud spend.
//!
//! The runtimes are reached through [`RuntimeEngine`] (an [`EngineApi`] that can also run its control loop), so tests drive the
//! router with recording fakes exactly like `product-engine.test.ts`.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use ratchet_core::enums::AgentsWhere;
use ratchet_core::ids::{AgentId, OptimizationId};
use ratchet_core::ports::{
    CancellationToken, Clock, EngineApi, EngineCreateOptimizationInput, EngineEvent, FileDiff,
    GitHost, PortError, ProposeTemplateInput, ProposeTemplateResult, PullRequestRef,
    SandboxActivity, SandboxInput, SandboxOutputs, SandboxPlan, SandboxState,
    SandboxTeardownReceipt,
};
use ratchet_core::{Calibration, EffectiveProtected};
use ratchet_github::{GitHubGitHost, GitHubGitHostOptions, friendly_missing, slug_of};
use ratchet_ledger::Ledger;
use ratchet_sandbox::images::{ImageOverride, RatchetImageKind};
use ratchet_sandbox::{
    Ec2ImageLookup, ResolveImageArgs, ResolvedImages, region_for_optimization, resolve_image_refs,
};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

use super::harnesses::HarnessCache;
use super::live_config::{LivePaths, effective_paths, secret_source};
use crate::engine::Engine;
use crate::errors::{EngineError, EngineErrorCode, EngineResult};
use crate::runtime::{
    HybridOptions, LiveOptions, SimulationOptions, SimulationProject, create_hybrid_runtime,
    create_simulation_runtime,
};
use crate::settings::EngineSettings;

type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// An engine the router can drive: the [`EngineApi`] plus its control loop.
#[async_trait]
pub trait RuntimeEngine: EngineApi {
    /// Tick until `cancel` fires.
    async fn run(&self, cancel: CancellationToken);

    /// The setup agent that drafts environments (docs/SIMPLIFIED_MODEL.md, "Dynamic environments"); `None` for an
    /// engine that has none.
    fn environment_drafter(&self) -> Option<Arc<dyn ratchet_env::EnvironmentDrafter>> {
        None
    }
}

#[async_trait]
impl RuntimeEngine for Engine {
    async fn run(&self, cancel: CancellationToken) {
        Engine::run(self, Some(cancel)).await;
    }

    fn environment_drafter(&self) -> Option<Arc<dyn ratchet_env::EnvironmentDrafter>> {
        self.ports.environment.clone()
    }
}

/// What the router needs from a real-run runtime.
#[derive(Clone)]
pub struct LiveHandle {
    pub engine: Arc<dyn RuntimeEngine>,
    /// The GitHub remote the ledger records for this runtime's optimizations.
    pub repo_url: Option<String>,
    /// What still blocks a real run (empty = ready).
    pub missing: Vec<String>,
    pub agents_where: AgentsWhere,
    /// Why there is no cloud runner image (fine for local agents, a blocker for "Cloud (unattended)"). Set by the router.
    pub runner_problem: Option<String>,
    pub disposer: Arc<dyn Fn() -> BoxFut<()> + Send + Sync>,
}

/// What the router needs from a demo (simulation) runtime.
#[derive(Clone)]
pub struct SimHandle {
    pub engine: Arc<dyn RuntimeEngine>,
    pub repo_url: String,
    pub base_ref: String,
    pub git: Arc<dyn GitHost>,
    pub disposer: Arc<dyn Fn() -> BoxFut<()> + Send + Sync>,
}

pub type LiveFactory = Arc<dyn Fn(HybridOptions) -> BoxFut<EngineResult<LiveHandle>> + Send + Sync>;
pub type SimFactory =
    Arc<dyn Fn(SimulationOptions) -> BoxFut<EngineResult<SimHandle>> + Send + Sync>;

/// Why an image lookup failed: a lookup error says which image to build (shown as is), anything else is summarized.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageLookupFailure {
    Lookup(String),
    Other(String),
}

pub struct ImageQuery {
    pub app_version: String,
    pub region: String,
    pub need: Vec<RatchetImageKind>,
    pub runner_override: Option<String>,
    pub bench_override: Option<String>,
    pub profile: Option<String>,
}

pub type ImageResolver =
    Arc<dyn Fn(ImageQuery) -> BoxFut<Result<ResolvedImages, ImageLookupFailure>> + Send + Sync>;

/// The settings the router reads (the daemon's `SettingsStore` implements it); secrets are looked up here, in the daemon.
pub trait LiveSettingsSource: Send + Sync {
    fn live(&self) -> LiveSnapshot;
    /// A secret by setting name (`githubToken`, `anthropicApiKey`, `openaiApiKey`): the store or the environment.
    fn secret(&self, name: &str) -> Option<String>;
}

/// The non-secret live settings (`SettingsStore.live`).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LiveSnapshot {
    pub aws_profile: String,
    /// Empty = the default region.
    pub aws_region: String,
    pub runner_image_ref: String,
    pub bench_image_ref: String,
    pub tofu_binary: String,
    pub module_dir: String,
}

/// What the production engine is built from.
#[derive(Clone)]
pub struct LiveAppConfig {
    pub settings: Arc<dyn LiveSettingsSource>,
    /// Where per-sandbox OpenTofu state, the git cache, local agent worktrees and scratch dirs live (`<support>/sandboxes`).
    pub state_root: PathBuf,
    /// What empty tofu settings resolve to (bundled infra/aws in a packaged app).
    pub defaults: LivePaths,
    /// The detected agent CLIs, cached.
    pub harnesses: Arc<HarnessCache>,
    /// The app version the published AMIs are matched against (`ratchet:image_version`).
    pub app_version: Option<String>,
    /// The parsed ratchet.yaml (as JSON) of a repo the user looked at in step 1 (for `environment.region`).
    pub config_for: Arc<dyn Fn(&str) -> Option<serde_json::Value> + Send + Sync>,
    /// The environment AWS credentials are looked for in (`AWS_ACCESS_KEY_ID`). Default: the process environment.
    pub env: Option<HashMap<String, String>>,
    /// Test seams.
    pub create_runtime: Option<LiveFactory>,
    pub resolve_images: Option<ImageResolver>,
}

#[derive(Clone)]
pub struct ProductOptions {
    pub ledger: Ledger,
    /// Monorepo root (simulation needs `examples/` and `node_modules`).
    pub repo_root: PathBuf,
    /// Scratch root for simulation runs.
    pub work_root: PathBuf,
    /// Idle window of the automatic environment in demos, ms (default 30 min).
    pub sim_idle_ms: Option<i64>,
    /// Needed for real runs; without it only demos work.
    pub live: Option<LiveAppConfig>,
    pub clock: Option<Arc<dyn Clock>>,
    /// Test seam: replaces `create_simulation_runtime`.
    pub create_simulation: Option<SimFactory>,
}

struct LiveEntry {
    handle: LiveHandle,
    stale: bool,
    key: String,
}

struct Inner {
    opts: ProductOptions,
    events: broadcast::Sender<EngineEvent>,
    attached: Mutex<HashSet<String>>,
    active_by: Mutex<HashMap<String, Vec<OptimizationId>>>,
    last_active: Mutex<(bool, String)>,
    /// Serializes runtime construction (a build takes seconds and must not run twice for one key).
    build: tokio::sync::Mutex<()>,
    sims: Mutex<HashMap<SimulationProject, SimHandle>>,
    sim_roots: Mutex<HashMap<String, SimulationProject>>,
    lives: Mutex<HashMap<String, LiveEntry>>,
    git_hosts: Mutex<HashMap<String, Arc<GitHubGitHost>>>,
    cancel: CancellationToken,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    disposed: AtomicBool,
    invalidations: std::sync::atomic::AtomicU64,
}

/// The product engine. Cheap to clone.
#[derive(Clone)]
pub struct ProductEngine(Arc<Inner>);

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

fn err(code: EngineErrorCode, msg: impl Into<String>) -> EngineError {
    EngineError::new(code, msg)
}

fn port(e: PortError) -> EngineError {
    match e {
        PortError::Cancelled => err(EngineErrorCode::Aborted, "cancelled"),
        other => err(EngineErrorCode::Port, other.to_string()),
    }
}

pub const NO_REAL_RUNS: &str =
    "Real runs are not set up in this build. Use a demo, or start the app normally.";
pub const SIGN_IN_FIRST: &str = "Sign in with GitHub first (Settings).";

fn default_sim_factory() -> SimFactory {
    Arc::new(|opts| {
        Box::pin(async move {
            let rt = Arc::new(create_simulation_runtime(opts).await?);
            let engine = Arc::new(rt.engine.clone());
            let r = rt.clone();
            Ok(SimHandle {
                engine,
                repo_url: rt.repo_url.clone(),
                base_ref: rt.base_ref.clone(),
                git: rt.git.clone(),
                disposer: Arc::new(move || {
                    let r = r.clone();
                    Box::pin(async move { r.dispose().await })
                }),
            })
        })
    })
}

fn default_live_factory() -> LiveFactory {
    Arc::new(|opts| {
        Box::pin(async move {
            let rt = Arc::new(create_hybrid_runtime(opts).await?);
            let r = rt.clone();
            Ok(LiveHandle {
                engine: Arc::new(rt.engine.clone()),
                repo_url: rt.repo_url.clone(),
                missing: rt.missing.clone(),
                agents_where: rt.agents_where,
                runner_problem: None,
                disposer: Arc::new(move || {
                    let r = r.clone();
                    Box::pin(async move { r.dispose().await })
                }),
            })
        })
    })
}

fn default_image_resolver() -> ImageResolver {
    Arc::new(|q| {
        Box::pin(async move {
            let lookup = Ec2ImageLookup::connect(&q.region, q.profile.as_deref()).await;
            let mut args = ResolveImageArgs::new(q.app_version, q.region);
            args.need = Some(q.need);
            args.override_ids = ImageOverride {
                runner: q.runner_override,
                bench: q.bench_override,
            };
            resolve_image_refs(&lookup, &args)
                .await
                .map_err(|e| match e {
                    ratchet_sandbox::Error::ImageLookup(l) => {
                        ImageLookupFailure::Lookup(l.to_string())
                    }
                    other => ImageLookupFailure::Other(other.to_string()),
                })
        })
    })
}

impl ProductEngine {
    pub fn new(opts: ProductOptions) -> Self {
        let (events, _) = broadcast::channel(1024);
        let this = Self(Arc::new(Inner {
            opts,
            events,
            attached: Mutex::default(),
            active_by: Mutex::default(),
            last_active: Mutex::new((false, String::new())),
            build: tokio::sync::Mutex::new(()),
            sims: Mutex::default(),
            sim_roots: Mutex::default(),
            lives: Mutex::default(),
            git_hosts: Mutex::default(),
            cancel: CancellationToken::new(),
            tasks: Mutex::default(),
            disposed: AtomicBool::new(false),
            invalidations: std::sync::atomic::AtomicU64::new(0),
        }));
        // An environment that outlived the previous run of the app must still be watched and removed after its idle window.
        if this.0.opts.live.is_some() {
            let me = this.clone();
            let task = tokio::spawn(async move {
                let applied =
                    me.0.opts
                        .ledger
                        .list_sandboxes()
                        .await
                        .map(|rows| {
                            rows.iter().any(|r| {
                                r.provider == ratchet_core::enums::SandboxProviderKind::Aws
                                    && r.status == ratchet_core::enums::SandboxStatus::Applied
                            })
                        })
                        .unwrap_or(false);
                if applied {
                    let _ = me.live(None).await;
                }
            });
            lock(&this.0.tasks).push(task);
        }
        this
    }

    fn ledger(&self) -> &Ledger {
        &self.0.opts.ledger
    }

    pub fn mode(&self) -> ratchet_core::enums::Mode {
        ratchet_core::enums::Mode::Live
    }

    /// True while any runtime has a run active.
    pub fn run_active(&self) -> bool {
        lock(&self.0.last_active).0
    }

    /// Every runtime's events, with `run-active` merged across runtimes.
    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.0.events.subscribe()
    }

    // ------------------------------------------------------------------ routing

    fn sim_project_for(&self, repo: &str) -> SimulationProject {
        lock(&self.0.sim_roots)
            .get(repo)
            .copied()
            .or_else(|| lock(&self.0.sims).keys().next().copied())
            .unwrap_or(SimulationProject::Search)
    }

    async fn sim(&self, project: SimulationProject) -> EngineResult<SimHandle> {
        if let Some(h) = lock(&self.0.sims).get(&project) {
            return Ok(h.clone());
        }
        let _build = self.0.build.lock().await;
        if let Some(h) = lock(&self.0.sims).get(&project) {
            return Ok(h.clone());
        }
        let root = self.0.opts.work_root.join(project.as_str());
        tokio::fs::create_dir_all(&root).await.map_err(|e| {
            err(
                EngineErrorCode::Internal,
                format!("cannot create {}: {e}", root.display()),
            )
        })?;
        let mut o =
            SimulationOptions::new(self.ledger().clone(), root, self.0.opts.repo_root.clone());
        o.project = project;
        o.clock = self.0.opts.clock.clone();
        o.settings = EngineSettings::default();
        // The demo environment is automatic too: created at the first run, removed after the idle window, with a receipt.
        o.settings.sandbox.auto = Some(true);
        let id = format!("ratchet-sim-{}", project.as_str());
        o.settings.sandbox.sandbox_id = Some(Arc::new(move |_region| id.clone()));
        if let Some(ms) = self.0.opts.sim_idle_ms.filter(|m| *m > 0) {
            o.settings.sandbox.idle_ms = Some(ms);
        }
        let factory = self
            .0
            .opts
            .create_simulation
            .clone()
            .unwrap_or_else(default_sim_factory);
        let handle = factory(o).await?;
        lock(&self.0.sims).insert(project, handle.clone());
        lock(&self.0.sim_roots).insert(handle.repo_url.clone(), project);
        self.attach(&format!("sim:{}", project.as_str()), &handle.engine);
        Ok(handle)
    }

    /// The runtime for one GitHub repo (`None`: no repo, only to watch/remove an environment that is already there).
    async fn live(&self, slug: Option<&str>) -> EngineResult<LiveHandle> {
        let Some(cfg) = self.0.opts.live.clone() else {
            return Err(err(EngineErrorCode::NotConfigured, NO_REAL_RUNS));
        };
        let s = cfg.settings.live();
        let token = cfg.settings.secret("githubToken");
        if slug.is_some() && token.is_none() {
            return Err(err(EngineErrorCode::NotConfigured, SIGN_IN_FIRST));
        }
        let fallback = if s.aws_region.is_empty() {
            "us-east-1".to_owned()
        } else {
            s.aws_region.clone()
        };
        let repo_config = slug
            .and_then(|sl| (cfg.config_for)(sl))
            .unwrap_or(serde_json::Value::Null);
        let region = region_for_optimization(&repo_config, Some(&fallback))
            .map_err(|e| err(EngineErrorCode::InvalidConfig, e.to_string()))?;
        let key = [
            slug.unwrap_or("*"),
            &region,
            &s.aws_profile,
            &s.runner_image_ref,
            &s.bench_image_ref,
            &s.tofu_binary,
            &s.module_dir,
            if token.is_some() { "tok" } else { "" },
        ]
        .join("|");
        let slot = slug.unwrap_or("*").to_owned();

        let _build = self.0.build.lock().await;
        let old = {
            let lives = lock(&self.0.lives);
            lives
                .get(&slot)
                .map(|e| (e.handle.clone(), e.stale, e.key.clone()))
        };
        if let Some((handle, stale, old_key)) = old {
            if !stale && old_key == key {
                return Ok(handle);
            }
            if handle.engine.run_active() {
                // a run is active on it: keep it until it finishes, then the next call rebuilds
                return Ok(handle);
            }
            let disposer = handle.disposer.clone();
            lock(&self.0.lives).remove(&slot);
            tokio::spawn(async move { disposer().await });
        }
        let handle = self.build_live(&cfg, slug, &region, token).await?;
        lock(&self.0.lives).insert(
            slot.clone(),
            LiveEntry {
                handle: handle.clone(),
                stale: false,
                key,
            },
        );
        self.attach(&format!("live:{slot}"), &handle.engine);
        Ok(handle)
    }

    async fn build_live(
        &self,
        cfg: &LiveAppConfig,
        slug: Option<&str>,
        region: &str,
        token: Option<String>,
    ) -> EngineResult<LiveHandle> {
        let s = cfg.settings.live();
        let paths = effective_paths(&s.tofu_binary, &s.module_dir, &cfg.defaults);
        tokio::fs::create_dir_all(&cfg.state_root)
            .await
            .map_err(|e| {
                err(
                    EngineErrorCode::Internal,
                    format!("cannot create {}: {e}", cfg.state_root.display()),
                )
            })?;
        let detected = cfg.harnesses.get().await;
        let mut runner_image_ref = Some(s.runner_image_ref.clone()).filter(|r| !r.is_empty());
        let mut bench_image_ref = Some(s.bench_image_ref.clone()).filter(|r| !r.is_empty());
        // The AMIs are found by tag in the repo's region, never typed. Needs AWS credentials. The evaluator's (bench) image is
        // required; the cloud runner image only for "Cloud (unattended)" runs, so a miss there is noted but does not block local
        // agents.
        let has_env_creds = match &cfg.env {
            Some(e) => e.get("AWS_ACCESS_KEY_ID").is_some_and(|v| !v.is_empty()),
            None => std::env::var("AWS_ACCESS_KEY_ID").is_ok_and(|v| !v.is_empty()),
        };
        let app_version = cfg.app_version.clone().filter(|v| !v.is_empty());
        let can_look_up = app_version.is_some() && (!s.aws_profile.is_empty() || has_env_creds);
        let resolver = cfg
            .resolve_images
            .clone()
            .unwrap_or_else(default_image_resolver);
        let (runner_override, bench_override) = (runner_image_ref.clone(), bench_image_ref.clone());
        let find = |need: RatchetImageKind| ImageQuery {
            app_version: app_version.clone().unwrap_or_default(),
            region: region.to_owned(),
            need: vec![need],
            runner_override: runner_override.clone(),
            bench_override: bench_override.clone(),
            profile: Some(s.aws_profile.clone()).filter(|p| !p.is_empty()),
        };
        let mut image_problem: Option<String> = None;
        let mut runner_problem: Option<String> = None;
        if can_look_up && bench_image_ref.is_none() {
            match resolver(find(RatchetImageKind::Bench)).await {
                Ok(r) => bench_image_ref = r.bench,
                Err(ImageLookupFailure::Lookup(m)) => image_problem = Some(m),
                Err(ImageLookupFailure::Other(m)) => {
                    image_problem = Some(format!(
                        "Could not look up the evaluator image: {}",
                        m.lines().next().unwrap_or_default()
                    ));
                }
            }
        }
        if can_look_up && runner_image_ref.is_none() {
            match resolver(find(RatchetImageKind::Runner)).await {
                Ok(r) => runner_image_ref = r.runner,
                Err(ImageLookupFailure::Lookup(m)) => runner_problem = Some(m),
                Err(ImageLookupFailure::Other(m)) => {
                    runner_problem = Some(format!(
                        "Could not look up the cloud runner image: {}",
                        m.lines().next().unwrap_or_default()
                    ));
                }
            }
        }
        let settings = cfg.settings.clone();
        let mut live = LiveOptions::new(
            self.ledger().clone(),
            region,
            "ratchet-sandbox",
            paths.tofu_binary,
            paths.module_dir,
            cfg.state_root.clone(),
        );
        live.sandbox = None;
        live.github_repo = slug.map(str::to_owned);
        live.github_token = token;
        live.runner_image_ref = runner_image_ref.clone();
        live.bench_image_ref = bench_image_ref;
        live.app_version = cfg.app_version.clone();
        live.secrets = Some(secret_source(Arc::new(move |name| settings.secret(name))));
        live.aws_profile = Some(s.aws_profile.clone()).filter(|p| !p.is_empty());
        live.clock = self.0.opts.clock.clone();
        let mut options = HybridOptions::new(live);
        options.harnesses = Some(detected.harnesses);
        options.login_path = detected.login_path;
        let factory = cfg
            .create_runtime
            .clone()
            .unwrap_or_else(default_live_factory);
        let mut handle = factory(options).await?;
        if let Some(p) = image_problem {
            handle.missing.push(p);
        }
        if runner_problem.is_some() || runner_image_ref.is_none() {
            let reason = runner_problem.unwrap_or_else(|| {
                "The cloud runner image was not found in your AWS account (use \"This Mac\", or build the image)".to_owned()
            });
            handle.runner_problem = Some(reason);
        }
        Ok(handle)
    }

    /// Mark every real runtime stale (settings or sign-in changed): idle ones are rebuilt on next use, busy ones when they finish.
    pub fn invalidate(&self) {
        self.0.invalidations.fetch_add(1, Ordering::SeqCst);
        for e in lock(&self.0.lives).values_mut() {
            e.stale = true;
        }
    }

    /// How many times `invalidate` was called (settings or sign-in changed).
    pub fn invalidations(&self) -> u64 {
        self.0.invalidations.load(Ordering::SeqCst)
    }

    /// What still blocks a real run on this repo ("what's missing"), after building its runtime.
    pub async fn missing_for(&self, slug: &str) -> EngineResult<Vec<String>> {
        let rt = self.live(Some(slug)).await?;
        Ok(friendly_missing(&rt.missing))
    }

    // ------------------------------------------------------------------ events

    fn attach(&self, key: &str, engine: &Arc<dyn RuntimeEngine>) {
        if !lock(&self.0.attached).insert(key.to_owned()) {
            return;
        }
        let mut rx = engine.subscribe();
        let me = self.clone();
        let key = key.to_owned();
        let forward = tokio::spawn(async move {
            loop {
                match rx.recv().await {
                    Ok(EngineEvent::RunActive {
                        active,
                        optimization_ids,
                    }) => {
                        lock(&me.0.active_by).insert(
                            key.clone(),
                            if active { optimization_ids } else { Vec::new() },
                        );
                        let mut ids: Vec<OptimizationId> =
                            lock(&me.0.active_by).values().flatten().cloned().collect();
                        ids.sort();
                        ids.dedup();
                        let is_active = !ids.is_empty();
                        let sig = ids
                            .iter()
                            .map(OptimizationId::as_str)
                            .collect::<Vec<_>>()
                            .join(",");
                        {
                            let mut last = lock(&me.0.last_active);
                            if last.0 == is_active && last.1 == sig {
                                continue;
                            }
                            *last = (is_active, sig);
                        }
                        let _ = me.0.events.send(EngineEvent::RunActive {
                            active: is_active,
                            optimization_ids: ids,
                        });
                    }
                    Ok(e) => {
                        let _ = me.0.events.send(e);
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        });
        let engine = engine.clone();
        let cancel = self.0.cancel.child_token();
        let run = tokio::spawn(async move { engine.run(cancel).await });
        let mut t = lock(&self.0.tasks);
        t.push(forward);
        t.push(run);
    }

    // ------------------------------------------------------------------ routing by optimization / sandbox

    async fn engine_for_optimization(&self, id: &str) -> EngineResult<Arc<dyn RuntimeEngine>> {
        let repo = self
            .ledger()
            .get_optimization(&OptimizationId::new(id))
            .await?
            .map(|o| o.repo)
            .ok_or_else(|| {
                err(
                    EngineErrorCode::NotFound,
                    format!("optimization {id} not found"),
                )
            })?;
        match slug_of(&repo) {
            None => Ok(self.sim(self.sim_project_for(&repo)).await?.engine),
            Some(slug) => Ok(self.live(Some(&slug)).await?.engine),
        }
    }

    async fn engine_for_sandbox(&self, sandbox_id: &str) -> EngineResult<Arc<dyn RuntimeEngine>> {
        let row = self.ledger().get_sandbox(sandbox_id).await?;
        let mock = match &row {
            Some(r) => r.provider == ratchet_core::enums::SandboxProviderKind::Mock,
            None => sandbox_id.starts_with("ratchet-sim"),
        };
        if mock {
            let project = sandbox_id
                .strip_prefix("ratchet-sim-")
                .and_then(SimulationProject::parse)
                .or_else(|| lock(&self.0.sims).keys().next().copied())
                .unwrap_or(SimulationProject::Search);
            return Ok(self.sim(project).await?.engine);
        }
        let first = lock(&self.0.lives)
            .values()
            .next()
            .map(|e| e.handle.clone());
        match first {
            Some(h) => Ok(h.engine),
            None => Ok(self.live(None).await?.engine),
        }
    }

    async fn by_agent(&self, agent_id: &str) -> EngineResult<Arc<dyn RuntimeEngine>> {
        let opt = self
            .ledger()
            .get_agent(&AgentId::new(agent_id))
            .await?
            .map(|a| a.optimization_id)
            .ok_or_else(|| {
                err(
                    EngineErrorCode::NotFound,
                    format!("agent {agent_id} not found"),
                )
            })?;
        self.engine_for_optimization(opt.as_str()).await
    }

    // ------------------------------------------------------------------ the commands

    /// A real run: needs the GitHub slug, a signed-in user and a ready runtime.
    pub async fn create_optimization(
        &self,
        input: &EngineCreateOptimizationInput,
    ) -> EngineResult<OptimizationId> {
        let slug = slug_of(&input.repo_url).ok_or_else(|| {
            err(
                EngineErrorCode::InvalidInput,
                format!(
                    "{} is not a GitHub repository (owner/name).",
                    input.repo_url
                ),
            )
        })?;
        let rt = self.live(Some(&slug)).await?;
        let mut missing = friendly_missing(&rt.missing);
        let cloud = input.agents_where.unwrap_or(rt.agents_where) == AgentsWhere::Cloud;
        if cloud && let Some(p) = rt.runner_problem.clone() {
            missing.push(p);
        }
        if !missing.is_empty() {
            return Err(err(
                EngineErrorCode::NotConfigured,
                format!("Not ready: {}", missing.join("; ")),
            ));
        }
        // The runtime is scoped to this repo; its remote is what the ledger records.
        let mut i = input.clone();
        i.repo_url = rt
            .repo_url
            .clone()
            .unwrap_or_else(|| input.repo_url.clone());
        i.agents_where = Some(input.agents_where.unwrap_or(rt.agents_where));
        rt.engine.create_optimization(&i).await.map_err(port)
    }

    /// A demo goes to the simulation runtime of its project; it needs no repository, sign-in or AWS.
    pub async fn create_demo(
        &self,
        demo: &str,
        input: &EngineCreateOptimizationInput,
    ) -> EngineResult<OptimizationId> {
        let project = demo_project(demo).ok_or_else(|| {
            err(
                EngineErrorCode::InvalidInput,
                format!("unknown demo {demo}"),
            )
        })?;
        let rt = self.sim(project).await?;
        // The simulation's scripted agents are its runner port; there is no real local control to choose.
        let mut i = input.clone();
        i.agents_where = None;
        i.repo_url = rt.repo_url.clone();
        i.base_ref = rt.base_ref.clone();
        rt.engine.create_optimization(&i).await.map_err(port)
    }

    pub async fn start(&self, id: &str) -> EngineResult<()> {
        let e = self.engine_for_optimization(id).await?;
        e.start(&OptimizationId::new(id)).await.map_err(port)
    }

    pub async fn stop(&self, id: &str, reason: Option<&str>) -> EngineResult<()> {
        let e = self.engine_for_optimization(id).await?;
        e.stop(&OptimizationId::new(id), reason).await.map_err(port)
    }

    pub async fn pause_agent(&self, agent_id: &str) -> EngineResult<()> {
        let e = self.by_agent(agent_id).await?;
        e.pause_agent(&AgentId::new(agent_id)).await.map_err(port)
    }

    pub async fn resume_agent(&self, agent_id: &str) -> EngineResult<()> {
        let e = self.by_agent(agent_id).await?;
        e.resume_agent(&AgentId::new(agent_id)).await.map_err(port)
    }

    pub async fn message_agent(&self, agent_id: &str, text: &str) -> EngineResult<()> {
        let e = self.by_agent(agent_id).await?;
        e.message_agent(&AgentId::new(agent_id), text)
            .await
            .map_err(port)
    }

    pub async fn open_pull_request(&self, id: &str) -> EngineResult<PullRequestRef> {
        let e = self.engine_for_optimization(id).await?;
        e.open_pull_request(&OptimizationId::new(id))
            .await
            .map_err(port)
    }

    pub async fn teardown(&self, id: &str) -> EngineResult<()> {
        let e = self.engine_for_optimization(id).await?;
        e.teardown(&OptimizationId::new(id)).await.map_err(port)
    }

    pub async fn reconcile_costs(
        &self,
        id: &str,
    ) -> EngineResult<ratchet_core::ports::CostReconcileReport> {
        let e = self.engine_for_optimization(id).await?;
        e.reconcile_costs(&OptimizationId::new(id))
            .await
            .map_err(port)
    }

    /// `None` when the engine cannot say or the baseline has not run.
    pub async fn calibration(&self, id: &str) -> EngineResult<Option<Calibration>> {
        let e = self.engine_for_optimization(id).await?;
        match e.calibration(&OptimizationId::new(id)).await {
            Err(PortError::Unsupported(_)) => Ok(None),
            r => r.map_err(port),
        }
    }

    pub async fn protected_paths(&self, id: &str) -> EngineResult<EffectiveProtected> {
        let e = self.engine_for_optimization(id).await?;
        e.protected_paths(&OptimizationId::new(id))
            .await
            .map_err(port)
    }

    pub async fn propose_template(
        &self,
        input: &ProposeTemplateInput,
    ) -> EngineResult<ProposeTemplateResult> {
        let slug = slug_of(&input.repo_url).ok_or_else(|| {
            err(
                EngineErrorCode::InvalidInput,
                format!(
                    "{} is not a GitHub repository (owner/name).",
                    input.repo_url
                ),
            )
        })?;
        let rt = self.live(Some(&slug)).await?;
        let mut i = input.clone();
        i.repo_url = rt
            .repo_url
            .clone()
            .unwrap_or_else(|| input.repo_url.clone());
        rt.engine.propose_template(&i).await.map_err(|e| match e {
            PortError::Unsupported(_) => err(
                EngineErrorCode::Unavailable,
                "This engine cannot open template pull requests.",
            ),
            other => port(other),
        })
    }

    /// The setup agent for one GitHub repository: the user's own Claude Code / Codex on this machine, with `tofu plan` checked
    /// in their AWS account. Needs the GitHub sign-in and a signed-in agent CLI.
    pub async fn environment_drafter(
        &self,
        repo: &str,
    ) -> EngineResult<Arc<dyn ratchet_env::EnvironmentDrafter>> {
        let slug = slug_of(repo).ok_or_else(|| {
            err(
                EngineErrorCode::InvalidInput,
                format!("{repo} is not a GitHub repository (owner/name)."),
            )
        })?;
        let rt = self.live(Some(&slug)).await?;
        rt.engine.environment_drafter().ok_or_else(|| {
            err(
                EngineErrorCode::Unavailable,
                "No setup agent is available: sign in to Claude Code or Codex on this machine and connect GitHub (Settings).",
            )
        })
    }

    pub async fn sandbox_state(&self, sandbox_id: &str) -> EngineResult<SandboxState> {
        let e = self.engine_for_sandbox(sandbox_id).await?;
        e.sandbox_state(sandbox_id).await.map_err(port)
    }

    pub async fn plan_sandbox(&self, input: &SandboxInput) -> EngineResult<SandboxPlan> {
        let e = self.engine_for_sandbox(&input.sandbox_id).await?;
        e.plan_sandbox(input).await.map_err(port)
    }

    pub async fn apply_sandbox(&self, input: &SandboxInput) -> EngineResult<SandboxOutputs> {
        let e = self.engine_for_sandbox(&input.sandbox_id).await?;
        e.apply_sandbox(input).await.map_err(port)
    }

    pub async fn destroy_sandbox(&self, sandbox_id: &str) -> EngineResult<()> {
        let e = self.engine_for_sandbox(sandbox_id).await?;
        e.destroy_sandbox(sandbox_id).await.map_err(port)
    }

    /// The receipt of the teardown that covered this run; `None` when there is none or the optimization is unknown.
    pub async fn teardown_receipt(&self, id: &str) -> EngineResult<Option<SandboxTeardownReceipt>> {
        let Ok(e) = self.engine_for_optimization(id).await else {
            return Ok(None);
        };
        match e.teardown_receipt(&OptimizationId::new(id)).await {
            Err(PortError::Unsupported(_)) => Ok(None),
            r => r.map_err(port),
        }
    }

    pub async fn sandbox_activity(&self, sandbox_id: &str) -> EngineResult<SandboxActivity> {
        let e = self.engine_for_sandbox(sandbox_id).await?;
        e.sandbox_activity(sandbox_id).await.map_err(|e| match e {
            PortError::Unsupported(_) => err(
                EngineErrorCode::Unavailable,
                "This engine has no sandbox activity.",
            ),
            other => port(other),
        })
    }

    pub async fn end_sandbox(&self, sandbox_id: &str) -> EngineResult<SandboxTeardownReceipt> {
        let e = self.engine_for_sandbox(sandbox_id).await?;
        e.end_sandbox(sandbox_id).await.map_err(|e| match e {
            PortError::Unsupported(_) => err(
                EngineErrorCode::Unavailable,
                "This engine cannot end the sandbox.",
            ),
            other => port(other),
        })
    }

    /// The diff of one commit against the base: the demo's local git, or the GitHub repo with the signed-in token. `None` when
    /// there is no access to diff.
    pub async fn diff(
        &self,
        repo: &str,
        base_ref: &str,
        sha: &str,
    ) -> EngineResult<Option<Vec<FileDiff>>> {
        let Some(slug) = slug_of(repo) else {
            let rt = self.sim(self.sim_project_for(repo)).await?;
            return rt.git.diff(base_ref, sha).await.map(Some).map_err(port);
        };
        let Some(cfg) = &self.0.opts.live else {
            return Ok(None);
        };
        let Some(token) = cfg.settings.secret("githubToken") else {
            return Ok(None);
        };
        let existing = lock(&self.0.git_hosts).get(&slug).cloned();
        let git = match existing {
            Some(g) => g,
            None => {
                let mut o = GitHubGitHostOptions::new(slug.clone(), token);
                o.cache_dir = Some(
                    cfg.state_root
                        .join("git-cache-diff")
                        .join(slug.replace('/', "__")),
                );
                let g = Arc::new(
                    GitHubGitHost::new(o).map_err(|e| err(EngineErrorCode::Port, e.to_string()))?,
                );
                lock(&self.0.git_hosts).insert(slug, g.clone());
                g
            }
        };
        git.diff(base_ref, sha).await.map(Some).map_err(port)
    }

    /// Stop the control loops and dispose every runtime. Idempotent.
    pub async fn dispose(&self) {
        if self.0.disposed.swap(true, Ordering::SeqCst) {
            return;
        }
        self.0.cancel.cancel();
        let tasks: Vec<_> = lock(&self.0.tasks).drain(..).collect();
        for t in tasks {
            t.abort();
        }
        let sims: Vec<_> = lock(&self.0.sims)
            .drain()
            .map(|(_, h)| h.disposer)
            .collect();
        let lives: Vec<_> = lock(&self.0.lives)
            .drain()
            .map(|(_, e)| e.handle.disposer)
            .collect();
        for d in sims.into_iter().chain(lives) {
            d().await;
        }
    }
}

/// `demo-search` / `demo-cli` / `demo-forecast` -> its simulation project.
pub fn demo_project(demo: &str) -> Option<SimulationProject> {
    match demo {
        "demo-search" => Some(SimulationProject::Search),
        "demo-cli" => Some(SimulationProject::Cli),
        "demo-forecast" => Some(SimulationProject::Forecast),
        _ => None,
    }
}
