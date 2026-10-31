//! The Ratchet engine: creates an optimization, runs the baseline, launches agents, then drives the control loop
//! (`tick`) until a stop rule or the user ends it. All durable state is in the ledger (plus the small `engine_kv`
//! table in the same SQLite file), so a new `Engine` on the same ledger resumes where the old one died.
//!
//! Tick model: `tick()` is one pass over every active optimization and never blocks on a bench job. A bench job is
//! started in one tick and its outcome is collected in a later one. `settle()` awaits running bench jobs (for tests);
//! `run()` is `tick()` + `clock.sleep(tick_interval_ms)` in a loop.
//!
//! Port of `packages/engine/src/engine.ts`; the loop itself is in [`crate::tick`], teardown / pull request / templates
//! in [`crate::misc`] and the automatic sandbox in [`crate::sandbox_auto`].

use std::collections::{HashMap, HashSet};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use futures::future::join_all;
use ratchet_core::bench::calibrate::calibration as cal;
use ratchet_core::config::{Guardrail, RatchetConfig, format_config_error, parse_config};
use ratchet_core::enums::{
    AgentKind, AgentStatus, AgentsWhere, Mode, OptimizationStatus, SandboxProviderKind,
};
use ratchet_core::ids::{AgentId, OptimizationId};
use ratchet_core::ports::{
    AgentEvent, AgentEventType, AgentIdentity, BaselineMetrics, BenchJob, BenchJobPhase, Clock,
    EngineApi, EngineCreateOptimizationInput, EngineEvent, GitHost, PortError, PortResult,
    RunnerConfig, RunnerControl, RunnerHandle, SandboxEnvironment, SandboxInput, SandboxOutputs, SandboxPlan,
    SandboxState, SecretRef,
};
use ratchet_core::sandbox::{
    MemorySandboxInputStore, SandboxBlockers, SandboxInputStore, SandboxLifecycle,
    SandboxLifecycleOptions,
};
use ratchet_core::{
    Calibration, EffectiveProtected, NoiseFloorOptions, ProtectedConfigInput, calibrate_baseline,
    effective_protected, median, noise_floor as compute_noise_floor,
};
use ratchet_ledger::{
    AddAgentInput, CreateOptimizationInput as LedgerCreate, Ledger, Patch, RecordBaselineInput,
    ValueInput,
};
use tokio::sync::{broadcast, watch};

use crate::bench_stats::{metrics_from_stats, samples_of, stats_of};
use crate::cost::CostMeter;
use crate::deps::{Deps, ErrorEntry, ErrorLog};
use crate::errors::{EngineError, EngineErrorCode, EngineResult, assert_valid_id};
use crate::kv::{KvStore, k};
use crate::ports::EnginePorts;
use crate::sandbox_auto::AutoState;
use crate::settings::{EngineSettings, ResolvedSettings, resolve_settings};
use crate::tick::{RunLoop, Runtime};

pub struct EngineOptions {
    pub ledger: Ledger,
    pub mode: Mode,
    pub clock: Arc<dyn Clock>,
    pub ports: EnginePorts,
    pub settings: EngineSettings,
}

/// What `create_optimization` takes: the core input plus an optional caller-chosen id (validated: `[a-z0-9-]`).
#[derive(Debug, Clone, Default)]
pub struct CreateOptimizationInput {
    pub id: Option<String>,
    pub name: Option<String>,
    pub repo_url: String,
    pub base_ref: String,
    /// Path inside the repo, default `ratchet.yaml`.
    pub config_path: Option<String>,
    pub agents: Vec<AgentKind>,
    pub budget_usd: f64,
    pub time_limit_seconds: u64,
    pub plateau_evals: u32,
    pub sandbox_id: Option<String>,
    pub agents_where: Option<AgentsWhere>,
}

impl From<&EngineCreateOptimizationInput> for CreateOptimizationInput {
    fn from(i: &EngineCreateOptimizationInput) -> Self {
        Self {
            id: None,
            name: i.name.clone(),
            repo_url: i.repo_url.clone(),
            base_ref: i.base_ref.clone(),
            config_path: i.config_path.clone(),
            agents: i.agents.clone(),
            budget_usd: i.budget_usd,
            time_limit_seconds: i.time_limit_seconds,
            plateau_evals: i.plateau_evals,
            sandbox_id: i.sandbox_id.clone(),
            agents_where: i.agents_where,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct ResumeReport {
    /// Optimizations the engine picked up again (running, stopping, or interrupted while baselining).
    pub attached: Vec<String>,
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) fn live_agent_status(s: AgentStatus) -> bool {
    matches!(
        s,
        AgentStatus::Launching | AgentStatus::Running | AgentStatus::Paused
    )
}

pub(crate) fn terminal_agent_status(s: AgentStatus) -> bool {
    matches!(
        s,
        AgentStatus::Stopped | AgentStatus::Done | AgentStatus::Failed
    )
}

/// Marker the live runtime's lazy compute puts in front of its error while no sandbox is applied.
pub const NOT_CONFIGURED: &str = "not_configured";

pub fn is_not_configured(e: &PortError) -> bool {
    matches!(e, PortError::Failed(m) if m.starts_with(NOT_CONFIGURED))
}

pub(crate) fn oid(id: &str) -> OptimizationId {
    OptimizationId::new(id)
}

pub(crate) fn aid(id: &str) -> AgentId {
    AgentId::new(id)
}

pub(crate) fn event_type_str(t: AgentEventType) -> String {
    serde_json::to_value(t)
        .ok()
        .and_then(|v| v.as_str().map(str::to_owned))
        .unwrap_or_default()
}

/// The sandbox teardown guard: leftover agent identities and live tagged instances block a destroy.
struct EngineBlockers {
    ledger: Ledger,
    kv: KvStore,
    compute: Arc<dyn ratchet_core::ports::CloudCompute>,
}

#[async_trait]
impl SandboxBlockers for EngineBlockers {
    async fn blockers(
        &self,
        _sandbox_id: &str,
        optimizations: &[ratchet_core::rows::Optimization],
    ) -> Result<Vec<String>, PortError> {
        let fail = |e: EngineError| PortError::Failed(e.message);
        let mut reasons = Vec::new();
        for o in optimizations {
            let mut left = 0;
            for a in self
                .ledger
                .list_agents(&o.id)
                .await
                .map_err(|e| PortError::Failed(e.message))?
            {
                if self
                    .kv
                    .has(&k::identity(a.id.as_str()))
                    .await
                    .map_err(fail)?
                {
                    left += 1;
                }
            }
            if left > 0 {
                reasons.push(format!(
                    "optimization {} still has {left} agent identit{}: run its teardown first",
                    o.id,
                    if left == 1 { "y" } else { "ies" }
                ));
            }
        }
        match self.compute.list_tagged(None).await {
            Ok(all) => {
                let alive: Vec<_> = all
                    .iter()
                    .filter(|i| i.state != ratchet_core::ports::InstanceState::Terminated)
                    .collect();
                if !alive.is_empty() {
                    reasons.push(format!(
                        "{} tagged instance(s) still alive ({})",
                        alive.len(),
                        alive
                            .iter()
                            .map(|i| i.id.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ));
                }
            }
            // No compute without sandbox outputs: nothing can have been launched. Any other failure blocks (fail closed).
            Err(e) if is_not_configured(&e) => {}
            Err(e) => return Err(e),
        }
        Ok(reasons)
    }
}

#[doc(hidden)]
pub struct Inner {
    pub ledger: Ledger,
    pub kv: KvStore,
    pub clock: Arc<dyn Clock>,
    pub ports: EnginePorts,
    pub settings: Arc<ResolvedSettings>,
    pub mode: Mode,
    pub deps: Deps,
    pub meter: CostMeter,
    pub sandboxes: SandboxLifecycle,
    /// Optimizations whose `start()` is in flight (a sandbox may be created for them before their status changes).
    pub pending_starts: Mutex<HashSet<String>>,
    pub run_active: AtomicBool,
    pub events: broadcast::Sender<EngineEvent>,
    pub rt: Mutex<HashMap<String, Arc<Runtime>>>,
    pub starting: Mutex<HashSet<String>>,
    pub configs: Mutex<HashMap<String, Arc<RatchetConfig>>>,
    pub tick_lock: tokio::sync::Mutex<()>,
    pub run_loop: Mutex<Option<RunLoop>>,
    pub resumed: watch::Receiver<Option<ResumeReport>>,
    pub auto: AutoState,
}

/// The engine. Cheap to clone (a handle).
#[derive(Clone)]
pub struct Engine {
    pub(crate) i: Arc<Inner>,
}

impl std::ops::Deref for Engine {
    type Target = Inner;
    fn deref(&self) -> &Inner {
        &self.i
    }
}

impl Engine {
    /// Build the engine: settle sandbox rows a crash left half-way (they become `failed`), then re-attach in the
    /// background to whatever the ledger says was in flight (`ready()` resolves when that pass is done).
    pub async fn new(opts: EngineOptions) -> Engine {
        let settings = Arc::new(resolve_settings(opts.settings));
        let kv = KvStore::new(opts.ledger.clone());
        let errors = Arc::new(ErrorLog::new(opts.clock.clone(), settings.on_error.clone()));
        let deps = Deps {
            ledger: opts.ledger.clone(),
            kv: kv.clone(),
            clock: opts.clock.clone(),
            compute: opts.ports.compute.clone(),
            prices: opts.ports.prices.clone(),
            settings: settings.clone(),
            errors,
        };
        // The lifecycle reads remembered sandbox inputs synchronously: load them (they are written through `kv`).
        let inputs = Arc::new(MemorySandboxInputStore::default());
        if let Ok(rows) = kv.entries("sandbox:").await {
            for (key, value) in rows {
                if let Some(id) = key
                    .strip_prefix("sandbox:")
                    .and_then(|r| r.strip_suffix(":input"))
                    && let Ok(input) = serde_json::from_str::<SandboxInput>(&value)
                {
                    inputs.set(id, input);
                }
            }
        }
        let sandboxes = SandboxLifecycle::new(SandboxLifecycleOptions {
            ledger: Arc::new(opts.ledger.clone()),
            provider: opts.ports.sandbox.clone(),
            kind: if opts.mode == Mode::Live {
                SandboxProviderKind::Aws
            } else {
                SandboxProviderKind::Mock
            },
            blockers: Some(Arc::new(EngineBlockers {
                ledger: opts.ledger.clone(),
                kv: kv.clone(),
                compute: opts.ports.compute.clone(),
            })),
            inputs: Some(inputs),
        });
        let (events, _) = broadcast::channel(16_384);
        let (resumed_tx, resumed_rx) = watch::channel(None);
        let inner = Inner {
            meter: CostMeter::new(deps.clone()),
            ledger: opts.ledger,
            kv,
            clock: opts.clock,
            ports: opts.ports,
            settings,
            mode: opts.mode,
            deps,
            sandboxes,
            pending_starts: Mutex::new(HashSet::new()),
            run_active: AtomicBool::new(false),
            events,
            rt: Mutex::new(HashMap::new()),
            starting: Mutex::new(HashSet::new()),
            configs: Mutex::new(HashMap::new()),
            tick_lock: tokio::sync::Mutex::new(()),
            run_loop: Mutex::new(None),
            resumed: resumed_rx,
            auto: AutoState::default(),
        };
        let engine = Engine { i: Arc::new(inner) };
        // An apply / destroy that a crash cut short left unknown infrastructure: surface it as `failed`.
        match engine.sandboxes.recover().await {
            Ok(ids) => {
                for id in ids {
                    engine.report_error("interrupted by a restart", &format!("sandbox {id}"));
                }
            }
            Err(e) => engine.report_error(e, "sandbox recover"),
        }
        let e2 = engine.clone();
        tokio::spawn(async move {
            let report = match e2.resume().await {
                Ok(r) => r,
                Err(err) => {
                    e2.report_error(err, "resume");
                    ResumeReport::default()
                }
            };
            e2.sync_run_active().await;
            let _ = resumed_tx.send(Some(report));
        });
        engine
    }

    // ------------------------------------------------------------------ small public surface

    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Resolves once the constructor's `resume()` pass has re-attached to running optimizations.
    pub async fn ready(&self) -> ResumeReport {
        let mut rx = self.resumed.clone();
        loop {
            if let Some(r) = rx.borrow().clone() {
                return r;
            }
            if rx.changed().await.is_err() {
                return ResumeReport::default();
            }
        }
    }

    /// True while any optimization is starting, baselining, running or stopping: the machine should not sleep. The
    /// same signal is emitted as `EngineEvent::RunActive` whenever it flips.
    pub fn run_active(&self) -> bool {
        self.run_active.load(Ordering::SeqCst)
    }

    pub fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.events.subscribe()
    }

    /// Swallowed non-fatal errors (poll failures, price lookups, ...), newest last, capped.
    pub fn errors(&self) -> Vec<ErrorEntry> {
        self.deps.errors.entries()
    }

    /// Why the optimization stopped, once it has.
    pub async fn stop_reason(&self, optimization_id: &str) -> EngineResult<Option<String>> {
        self.kv.get(&k::stop_reason(optimization_id)).await
    }

    pub async fn teardown_report(
        &self,
        optimization_id: &str,
    ) -> EngineResult<Option<crate::misc::TeardownReport>> {
        self.kv.get_json(&k::teardown(optimization_id)).await
    }

    /// The report of the latest `reconcile_costs` run for the optimization.
    pub async fn cost_reconcile_report(
        &self,
        optimization_id: &str,
    ) -> EngineResult<Option<ratchet_core::ports::CostReconcileReport>> {
        self.kv.get_json(&k::reconcile(optimization_id)).await
    }

    pub fn ledger(&self) -> &Ledger {
        &self.ledger
    }

    pub(crate) fn report_error(&self, err: impl std::fmt::Display, context: &str) {
        self.deps.report_error(err, context);
    }

    pub(crate) fn emit(&self, e: EngineEvent) {
        let _ = self.events.send(e);
    }

    // ------------------------------------------------------------------ sandbox lifecycle

    pub async fn sandbox_state(&self, sandbox_id: &str) -> EngineResult<SandboxState> {
        Ok(self.sandboxes.state(sandbox_id).await?)
    }

    /// `tofu plan` (or the mock's): what an apply would change. Records the sandbox as `planned` the first time.
    pub async fn plan_sandbox(&self, input: &SandboxInput) -> EngineResult<SandboxPlan> {
        Ok(self.sandboxes.plan(input).await?)
    }

    /// planned -> applying -> applied (or failed); the contract outputs are stored in the ledger.
    pub async fn apply_sandbox(&self, input: &SandboxInput) -> EngineResult<SandboxOutputs> {
        self.release_auto(&input.sandbox_id).await?; // an explicit apply takes it out of automatic management
        self.apply_lifecycle(input).await
    }

    /// Lifecycle apply with the input remembered (so a later destroy runs with the same variables), also after a restart.
    pub(crate) async fn apply_lifecycle(
        &self,
        input: &SandboxInput,
    ) -> EngineResult<SandboxOutputs> {
        self.kv
            .set_json(&k::sandbox_input(&input.sandbox_id), input)
            .await?;
        Ok(self.sandboxes.apply(input).await?)
    }

    /// applied -> destroying -> destroyed. Refused while an optimization in the sandbox is baselining, running or
    /// stopping, while agent identities of its optimizations were not torn down, and while any tagged instance is alive.
    pub async fn destroy_sandbox(&self, sandbox_id: &str) -> EngineResult<()> {
        self.sandboxes.destroy(sandbox_id).await?;
        self.release_auto(sandbox_id).await
    }

    /// Live optimizations need an applied sandbox and are linked to it. In simulation an applied sandbox is linked
    /// when there is one, so the Environments screen and the ledger agree; otherwise the optimization stays unlinked.
    async fn sandbox_for_new_optimization(
        &self,
        requested: Option<&str>,
    ) -> EngineResult<Option<String>> {
        if let Some(r) = requested {
            assert_valid_id("sandbox id", r)?;
        }
        let row = match requested {
            Some(r) => self.ledger.get_sandbox(r).await?,
            None => None,
        };
        if self.settings.sandbox.auto {
            return Ok(row.map(|r| r.id.to_string())); // created (or re-created) at the first run start
        }
        if self.mode == Mode::Live {
            self.assert_sandbox_applied(requested).await?;
            return Ok(requested.map(str::to_owned));
        }
        Ok(row
            .filter(|r| r.status == ratchet_core::enums::SandboxStatus::Applied)
            .map(|r| r.id.to_string()))
    }

    /// Live mode only: an optimization may start only inside an applied sandbox.
    pub(crate) async fn assert_sandbox_applied(
        &self,
        sandbox_id: Option<&str>,
    ) -> EngineResult<()> {
        if self.mode != Mode::Live {
            return Ok(());
        }
        let Some(sandbox_id) = sandbox_id else {
            return Err(EngineError::new(
                EngineErrorCode::SandboxNotApplied,
                "live optimizations run inside a sandbox: apply one first (Environments)",
            ));
        };
        let row = self.ledger.get_sandbox(sandbox_id).await?;
        match row {
            Some(r) if r.status == ratchet_core::enums::SandboxStatus::Applied => Ok(()),
            other => Err(EngineError::new(
                EngineErrorCode::SandboxNotApplied,
                format!(
                    "sandbox {sandbox_id} is {}: apply it first (Environments)",
                    other.map_or("not set up".to_owned(), |r| r.status.to_string())
                ),
            )),
        }
    }

    // ------------------------------------------------------------------ create_optimization

    pub async fn create_optimization(
        &self,
        input: CreateOptimizationInput,
    ) -> EngineResult<String> {
        self.ready().await;
        self.validate_create_input(&input)?;
        let sandbox_id = self
            .sandbox_for_new_optimization(input.sandbox_id.as_deref())
            .await?;
        let id = input
            .id
            .clone()
            .or_else(|| self.settings.new_id.as_ref().map(|f| f()))
            .unwrap_or_else(|| format!("opt-{:08x}", rand::random::<u32>()));
        assert_valid_id("optimization id", &id)?;
        if self.ledger.get_optimization(&oid(&id)).await?.is_some() {
            return Err(EngineError::invalid_input(format!(
                "optimization {id} already exists"
            )));
        }

        let git = self.git_for(&input.repo_url);
        let config_path = input
            .config_path
            .clone()
            .unwrap_or_else(|| "ratchet.yaml".into());
        let (config_yaml, _) = self
            .read_from_base(&*git, &input.base_ref, &config_path, None)
            .await?;
        let config = match parse_config(&config_yaml) {
            Ok(c) => c,
            Err(errors) => {
                return Err(EngineError::new(
                    EngineErrorCode::InvalidConfig,
                    format!(
                        "{config_path} is invalid:\n{}",
                        errors
                            .iter()
                            .map(|e| format!("  - {}", format_config_error(e)))
                            .collect::<Vec<_>>()
                            .join("\n")
                    ),
                )
                .with_details(serde_json::to_value(&errors).unwrap_or_default()));
            }
        };
        // Fail early when the brief is missing, rather than after the baseline has cost money.
        let (_, brief) = self
            .read_from_base(
                &*git,
                &input.base_ref,
                &config_path,
                Some(&config.agent.instructions),
            )
            .await?;
        if brief.as_deref().is_none_or(|b| b.trim().is_empty()) {
            return Err(EngineError::new(
                EngineErrorCode::InvalidConfig,
                format!(
                    "agent.instructions file {} is missing or empty at {}",
                    config.agent.instructions, input.base_ref
                ),
            ));
        }

        let where_ = self.resolve_agents_where(input.agents_where)?;
        let now = self.clock.now();
        let slots = self.agent_slots(&id, &input.agents)?;
        let mut create = LedgerCreate::new(
            input.repo_url.clone(),
            input.base_ref.clone(),
            config.objective.metric.clone(),
            config.objective.direction,
            // Provisional when ratchet.yaml leaves it out: the calibrated threshold (kv `calibration`) replaces it once
            // the baseline ran.
            config.objective.min_gain.unwrap_or(cal::FALLBACK_GAIN),
            input.budget_usd,
            input.time_limit_seconds as i64,
            config_yaml.clone(),
        );
        create.id = Some(oid(&id));
        create.name = input.name.clone();
        create.status = Some(OptimizationStatus::Draft);
        create.created_at = Some(now);
        create.pack = config.pack.clone();
        create.objective_name = config.objective.name.clone();
        create.evaluator_kind = Some(config.evaluator.kind);
        create.value = config.objective.value.as_ref().map(|v| ValueInput {
            usd_per_unit: v.usd_per_unit,
            period: v.period,
            basis: v.basis,
            unit: v.unit.clone(),
        });
        create.sandbox_id = sandbox_id.as_deref().map(Into::into);
        let (plateau, where_str) = (input.plateau_evals.to_string(), where_.to_string());
        let (oid_, id2) = (oid(&id), id.clone());
        self.ledger
            .atomically(move |mut tx| {
                Box::pin(async move {
                    tx.create_optimization(create).await?;
                    for (agent_id, kind, branch) in slots {
                        let mut a = AddAgentInput::new(oid_.clone(), kind, branch);
                        a.id = Some(aid(&agent_id));
                        a.status = Some(AgentStatus::Pending);
                        a.created_at = Some(now);
                        tx.add_agent(a).await?;
                    }
                    tx.kv_set(&k::plateau(&id2), &plateau).await?;
                    tx.kv_set(&k::agents_where(&id2), &where_str).await?;
                    Ok(())
                })
            })
            .await?;
        self.emit(EngineEvent::Optimization {
            optimization_id: oid(&id),
            status: "draft".into(),
        });
        Ok(id)
    }

    /// Request > engine setting > `local` when a local runner control exists > `cloud`.
    fn resolve_agents_where(&self, requested: Option<AgentsWhere>) -> EngineResult<AgentsWhere> {
        let where_ = requested.or(self.settings.agents_where).unwrap_or(
            if self.ports.local_runners.is_some() {
                AgentsWhere::Local
            } else {
                AgentsWhere::Cloud
            },
        );
        if where_ == AgentsWhere::Local && self.ports.local_runners.is_none() {
            return Err(EngineError::invalid_input(
                "agents cannot run on this machine here: the engine has no local runner control (use agentsWhere: cloud)",
            ));
        }
        Ok(where_)
    }

    /// Where this optimization's agents run (optimizations created before the setting existed ran in the cloud).
    pub async fn agents_where(&self, optimization_id: &str) -> AgentsWhere {
        let stored = self
            .kv
            .get(&k::agents_where(optimization_id))
            .await
            .ok()
            .flatten();
        if stored.as_deref() == Some("local") && self.ports.local_runners.is_some() {
            AgentsWhere::Local
        } else {
            AgentsWhere::Cloud
        }
    }

    /// The runner control for an optimization: the local one for local agents, else the cloud (SSM) one.
    pub(crate) async fn runners_for(&self, optimization_id: &str) -> Arc<dyn RunnerControl> {
        if self.agents_where(optimization_id).await == AgentsWhere::Local
            && let Some(l) = &self.ports.local_runners
        {
            return l.clone();
        }
        self.ports.runners.clone()
    }

    fn validate_create_input(&self, input: &CreateOptimizationInput) -> EngineResult<()> {
        let bad = |m: &str| Err(EngineError::invalid_input(m));
        if let Some(id) = &input.id {
            assert_valid_id("optimization id", id)?;
        }
        if input.repo_url.is_empty() {
            return bad("repoUrl is required");
        }
        if input.base_ref.is_empty() {
            return bad("baseRef is required");
        }
        if input.agents.is_empty() {
            return bad("at least one agent is required");
        }
        if !input.budget_usd.is_finite() || input.budget_usd <= 0.0 {
            return bad("budgetUsd must be a positive number");
        }
        if input.time_limit_seconds == 0 {
            return bad("timeLimitSeconds must be a positive number");
        }
        if input.plateau_evals < 1 {
            return bad("plateauEvals must be an integer >= 1");
        }
        if let Some(p) = &input.config_path
            && normalize_inside(p).is_none()
        {
            return bad("configPath must be a path inside the repository");
        }
        Ok(())
    }

    /// `<kind>`, `<kind>-2`, ... so two agents of one kind get distinct branches `ratchet/<opt>/<slot>`.
    fn agent_slots(
        &self,
        opt_id: &str,
        kinds: &[AgentKind],
    ) -> EngineResult<Vec<(String, AgentKind, String)>> {
        let mut seen: HashMap<AgentKind, u32> = HashMap::new();
        kinds
            .iter()
            .map(|kind| {
                let n = seen.entry(*kind).or_default();
                *n += 1;
                let slot = if *n == 1 {
                    kind.to_string()
                } else {
                    format!("{kind}-{n}")
                };
                let agent_id = format!("{opt_id}-{slot}");
                assert_valid_id("agent id", &agent_id)?;
                Ok((agent_id, *kind, format!("ratchet/{opt_id}/{slot}")))
            })
            .collect()
    }

    pub(crate) fn git_for(&self, repo_url: &str) -> Arc<dyn GitHost> {
        self.ports.git.for_repo(repo_url)
    }

    pub(crate) fn scratch_root(&self) -> PathBuf {
        self.settings
            .scratch_dir
            .clone()
            .unwrap_or_else(std::env::temp_dir)
    }

    /// Check `base_ref` out into a scratch dir and read ratchet.yaml (and optionally the brief).
    pub(crate) async fn read_from_base(
        &self,
        git: &dyn GitHost,
        base_ref: &str,
        config_path: &str,
        brief_path: Option<&str>,
    ) -> EngineResult<(String, Option<String>)> {
        let root = self.scratch_root();
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|e| EngineError::internal(e.to_string()))?;
        let dir = tempfile::Builder::new()
            .prefix("ratchet-engine-")
            .tempdir_in(&root)
            .map_err(|e| EngineError::internal(e.to_string()))?;
        git.checkout(base_ref, dir.path()).await?;
        let config_yaml = read_inside(dir.path(), config_path).await.map_err(|e| {
            EngineError::new(
                EngineErrorCode::InvalidConfig,
                format!("cannot read {config_path} at {base_ref}: {e}"),
            )
        })?;
        let brief = match brief_path {
            Some(p) => read_inside(dir.path(), p).await.ok(),
            None => None,
        };
        Ok((config_yaml, brief))
    }

    // ------------------------------------------------------------------ start

    pub async fn start(&self, id: &str) -> EngineResult<()> {
        assert_valid_id("optimization id", id)?;
        self.ready().await;
        let opt = self.require_opt(id).await?;
        let resumable =
            opt.status == OptimizationStatus::Baselining && !lock(&self.starting).contains(id);
        if opt.status != OptimizationStatus::Draft && !resumable {
            return Err(EngineError::invalid_state(format!(
                "optimization {id} is {}; only a draft can be started",
                opt.status
            )));
        }
        lock(&self.pending_starts).insert(id.to_owned());
        self.sync_run_active().await;
        let r = async {
            // The sandbox is created lazily, at the first run start in a region, and reused while runs are active.
            if !resumable && self.settings.sandbox.auto {
                self.ensure_sandbox_for(id).await?;
            } else if !resumable
                && ratchet_env::environment_module_of(&(&*self.config(id).await?).into()).is_some()
            {
                self.emit(EngineEvent::Notice {
                    optimization_id: oid(id),
                    level: ratchet_core::ports::NoticeLevel::Warning,
                    text: "ratchet.yaml declares an environment, but this engine does not manage the sandbox itself (apply it by hand): the environment is not applied.".into(),
                });
            }
            let fresh = self.require_opt(id).await?;
            self.assert_sandbox_applied(fresh.sandbox_id.as_ref().map(|s| s.as_str()))
                .await?;
            self.begin_start(id).await
        }
        .await;
        lock(&self.pending_starts).remove(id);
        self.sync_run_active().await;
        r
    }

    pub(crate) async fn begin_start(&self, id: &str) -> EngineResult<()> {
        lock(&self.starting).insert(id.to_owned());
        let r = self.run_start(id).await;
        lock(&self.starting).remove(id);
        r
    }

    pub(crate) async fn run_start(&self, id: &str) -> EngineResult<()> {
        let rt = self.state(id);
        match self.run_start_inner(id, &rt).await {
            Ok(()) => Ok(()),
            Err(err) => {
                let status = self
                    .ledger
                    .get_optimization(&oid(id))
                    .await
                    .ok()
                    .flatten()
                    .map(|o| o.status);
                if rt.killing.load(Ordering::SeqCst)
                    || status.is_some_and(|s| {
                        s != OptimizationStatus::Baselining && s != OptimizationStatus::Draft
                    })
                {
                    return Ok(()); // stop() took over
                }
                self.report_error(&err, &format!("start {id}"));
                if let Err(e) = self
                    .execute_kill(
                        id,
                        &format!("start failed: {}", err.message),
                        OptimizationStatus::Failed,
                    )
                    .await
                {
                    self.report_error(e, &format!("cleanup {id}"));
                }
                Err(err)
            }
        }
    }

    async fn run_start_inner(&self, id: &str, rt: &Arc<Runtime>) -> EngineResult<()> {
        if self.require_opt(id).await?.status == OptimizationStatus::Draft {
            self.set_status(id, OptimizationStatus::Baselining).await?;
        }
        let config = self.config(id).await?;

        let baseline_value = self.ensure_baseline(id, &config, rt).await?;
        self.assert_alive(id).await?;

        if self.kv.get_number(&k::deadline(id)).await?.is_none() {
            let limit = self.require_opt(id).await?.time_limit_s;
            self.kv
                .set(
                    &k::deadline(id),
                    &(self.clock.now() + limit * 1000).to_string(),
                )
                .await?;
        }
        let brief = self.load_brief(id, &config).await?;
        for agent in self.ledger.list_agents(&oid(id)).await? {
            self.assert_alive(id).await?;
            self.launch_agent(id, agent.id.as_str(), &config, &brief, baseline_value)
                .await?;
        }
        self.assert_alive(id).await?;
        self.set_status(id, OptimizationStatus::Running).await?;
        Ok(())
    }

    pub(crate) async fn assert_alive(&self, id: &str) -> EngineResult<()> {
        let rt = self.state(id);
        let status = self
            .ledger
            .get_optimization(&oid(id))
            .await?
            .map(|o| o.status);
        if rt.killing.load(Ordering::SeqCst)
            || matches!(
                status,
                Some(
                    OptimizationStatus::Stopping
                        | OptimizationStatus::Stopped
                        | OptimizationStatus::Failed
                )
            )
        {
            return Err(EngineError::new(
                EngineErrorCode::Aborted,
                format!("optimization {id} was stopped"),
            ));
        }
        Ok(())
    }

    async fn ensure_baseline(
        &self,
        id: &str,
        config: &Arc<RatchetConfig>,
        rt: &Arc<Runtime>,
    ) -> EngineResult<f64> {
        let opt = self.require_opt(id).await?;
        let mut baseline = self.ledger.get_baseline(&oid(id)).await?;
        if baseline.is_none() {
            let sha = self.base_sha(&opt).await?;
            let outcome = self
                .ports
                .bench
                .run(
                    &BenchJob {
                        optimization_id: oid(id),
                        candidate_id: None,
                        sha,
                        phase: BenchJobPhase::Baseline,
                        repeats: config.evaluator.repeats.unwrap_or(cal::BASELINE_REPEATS),
                        trace: None,
                    },
                    config,
                    None,
                    Some(&rt.abort),
                )
                .await?;
            self.assert_alive(id).await?;
            let samples = samples_of(&outcome).map_err(EngineError::internal)?;
            let stats = stats_of(&outcome).map_err(EngineError::internal)?;
            let command_guardrails: HashSet<String> = config
                .guardrails
                .iter()
                .filter(|g| matches!(g, Guardrail::Command(_)))
                .map(Guardrail::name)
                .collect();
            let broken: Vec<_> = outcome
                .guardrails
                .iter()
                .filter(|g| !g.passed && command_guardrails.contains(&g.name))
                .collect();
            if !broken.is_empty() {
                return Err(EngineError::invalid_state(format!(
                    "baseline fails its own guardrails: {}",
                    broken
                        .iter()
                        .map(|g| match &g.detail {
                            Some(d) if !d.is_empty() => format!("{} ({d})", g.name),
                            _ => g.name.clone(),
                        })
                        .collect::<Vec<_>>()
                        .join("; ")
                )));
            }
            let floor = compute_noise_floor(&samples, self.noise_options(config))
                .map_err(|e| EngineError::internal(e.to_string()))?;
            baseline = Some(
                self.ledger
                    .record_baseline(RecordBaselineInput {
                        optimization_id: oid(id),
                        samples,
                        noise_floor: floor,
                        value: Some(stats.value),
                        stats,
                        recorded_at: Some(self.clock.now()),
                    })
                    .await?,
            );
            if let Some(c) = self.calibration_for(id, config).await? {
                self.emit(EngineEvent::Notice {
                    optimization_id: oid(id),
                    level: ratchet_core::ports::NoticeLevel::Info,
                    text: format!("Calibrated from the baseline: {}", c.explanation),
                });
            }
        }
        let baseline = baseline.expect("baseline recorded above");
        if let Some(trace) = &config.evaluator.confirm_trace
            && self.settings.confirm_baseline
            && !self.kv.has(&k::confirm_best(id)).await?
        {
            let sha = self.base_sha(&opt).await?;
            let confirm = self
                .ports
                .bench
                .run(
                    &BenchJob {
                        optimization_id: oid(id),
                        candidate_id: None,
                        sha,
                        phase: BenchJobPhase::Confirm,
                        repeats: self.repeats_for(id, config).await?,
                        trace: Some(trace.clone()),
                    },
                    config,
                    Some(&BaselineMetrics {
                        metrics: metrics_from_stats(&baseline.stats),
                    }),
                    Some(&rt.abort),
                )
                .await?;
            self.assert_alive(id).await?;
            let samples = samples_of(&confirm).map_err(EngineError::internal)?;
            self.kv
                .set(
                    &k::confirm_best(id),
                    &ratchet_core::numfmt::js_number_to_string(
                        median(&samples).map_err(|e| EngineError::internal(e.to_string()))?,
                    ),
                )
                .await?;
        }
        Ok(baseline.value)
    }

    /// `base_ref` resolved to a sha once and remembered, so baseline, confirm-baseline and diffs all use the same commit.
    pub(crate) async fn base_sha(
        &self,
        opt: &ratchet_core::rows::Optimization,
    ) -> EngineResult<String> {
        let key = k::base_sha(opt.id.as_str());
        if let Some(known) = self.kv.get(&key).await? {
            return Ok(known);
        }
        let sha = self.git_for(&opt.repo).resolve_ref(&opt.base_ref).await?;
        self.kv.set(&key, &sha).await?;
        Ok(sha)
    }

    async fn load_brief(&self, id: &str, config: &RatchetConfig) -> EngineResult<String> {
        let opt = self.require_opt(id).await?;
        let git = self.git_for(&opt.repo);
        let brief = self
            .read_from_base(
                &*git,
                &opt.base_ref,
                "ratchet.yaml",
                Some(&config.agent.instructions),
            )
            .await
            .ok()
            .and_then(|(_, b)| b);
        match brief {
            Some(b) if !b.trim().is_empty() => Ok(b),
            _ => Err(EngineError::new(
                EngineErrorCode::InvalidConfig,
                format!(
                    "agent brief {} is missing at {}",
                    config.agent.instructions, opt.base_ref
                ),
            )),
        }
    }

    /// Idempotent, resumable launch: `launch_step` in the kv says how far this agent got.
    async fn launch_agent(
        &self,
        opt_id: &str,
        agent_id: &str,
        config: &Arc<RatchetConfig>,
        brief: &str,
        baseline_value: f64,
    ) -> EngineResult<()> {
        let opt = self.require_opt(opt_id).await?;
        let agent = self.require_agent(agent_id).await?;
        let compute = &self.ports.compute;
        let runners = self.runners_for(opt_id).await;
        // Local agents use the user's own CLI login on this machine: no cloud identity, secrets or runner instance.
        let local = self.agents_where(opt_id).await == AgentsWhere::Local;
        let git = self.git_for(&opt.repo);
        let step_key = k::launch(agent_id);
        let mut step = self.kv.get_i64(&step_key).await?.unwrap_or(0);
        macro_rules! done {
            ($n:expr) => {{
                step = $n;
                self.kv.set(&step_key, &step.to_string()).await?;
            }};
        }
        if step >= 5 {
            return Ok(());
        }
        if agent.status == AgentStatus::Pending {
            self.ledger
                .set_agent_status(&aid(agent_id), AgentStatus::Launching, Patch::Keep)
                .await?;
        }

        if !local && step < 1 {
            let identity = compute
                .create_agent_identity(&oid(opt_id), &aid(agent_id))
                .await?;
            self.kv.set_json(&k::identity(agent_id), &identity).await?;
            done!(1);
        }
        let identity: Option<AgentIdentity> = self.kv.get_json(&k::identity(agent_id)).await?;
        if !local && identity.is_none() {
            return Err(EngineError::invalid_state(format!(
                "identity for {agent_id} is missing"
            )));
        }

        if !local && step < 2 {
            let mut refs: Vec<SecretRef> = Vec::new();
            let names = self
                .settings
                .secret_names
                .get(&agent.kind)
                .cloned()
                .unwrap_or_default();
            for name in names {
                let value = (self.settings.secrets)(agent.kind, &name);
                match value {
                    Some(v) if !v.is_empty() => {
                        refs.push(
                            compute
                                .put_secret(&oid(opt_id), &aid(agent_id), &name, &v)
                                .await?,
                        );
                    }
                    _ => {
                        return Err(EngineError::new(
                            EngineErrorCode::MissingSecret,
                            format!("secret {name} for {} is not available", agent.kind),
                        ));
                    }
                }
            }
            self.kv.set_json(&k::secrets(agent_id), &refs).await?; // refs only, never values
            done!(2);
        }
        let secrets: Vec<SecretRef> = self
            .kv
            .get_json(&k::secrets(agent_id))
            .await?
            .unwrap_or_default();

        if step < 3 {
            git.create_branch(&self.base_sha(&opt).await?, &agent.branch)
                .await?;
            done!(3);
        }
        if local && step < 4 {
            self.ledger
                .set_agent_status(
                    &aid(agent_id),
                    AgentStatus::Launching,
                    Patch::Set(format!("local:{agent_id}")),
                )
                .await?;
            done!(4);
        }
        if !local && step < 4 {
            let instance = compute
                .launch_runner(&ratchet_core::ports::RunnerLaunchSpec {
                    optimization_id: oid(opt_id),
                    agent_id: aid(agent_id),
                    kind: agent.kind,
                    instance_type: config
                        .agent
                        .allowed_instance_types
                        .first()
                        .cloned()
                        .unwrap_or_default(),
                    image_ref: self.settings.runner_image_ref.clone(),
                    identity: identity.clone().expect("identity checked above"),
                    secrets: secrets.clone(),
                    repo_url: opt.repo.clone(),
                    branch: agent.branch.clone(),
                })
                .await?;
            self.ledger
                .set_agent_status(
                    &aid(agent_id),
                    AgentStatus::Launching,
                    Patch::Set(instance.id.clone()),
                )
                .await?;
            done!(4);
        }
        if step < 5 {
            let fresh = self.require_agent(agent_id).await?;
            let cal = self.calibration_for(opt_id, config).await?;
            let deadline = match self.kv.get_i64(&k::deadline(opt_id)).await? {
                Some(d) => d,
                None => self.clock.now() + opt.time_limit_s * 1000,
            };
            runners
                .start(
                    &handle(&fresh),
                    &RunnerConfig {
                        optimization_id: oid(opt_id),
                        agent_id: aid(agent_id),
                        kind: agent.kind,
                        repo_url: opt.repo.clone(),
                        branch: agent.branch.clone(),
                        base_ref: opt.base_ref.clone(),
                        config: (**config).clone(),
                        brief: brief.to_owned(),
                        baseline_value,
                        min_gain: cal.map(|c| c.min_gain),
                        budget_usd: opt.budget_usd,
                        deadline_at: deadline,
                    },
                )
                .await?;
            self.ledger
                .set_agent_status(&aid(agent_id), AgentStatus::Running, Patch::Keep)
                .await?;
            done!(5);
        }
        let _ = step;
        Ok(())
    }

    // ------------------------------------------------------------------ stop / kill switch

    /// Kill switch. Order matters: revoke identities first, then stop runners, then terminate instances.
    pub async fn stop(&self, id: &str, reason: Option<&str>) -> EngineResult<()> {
        assert_valid_id("optimization id", id)?;
        self.ready().await;
        let opt = self.require_opt(id).await?;
        match opt.status {
            OptimizationStatus::Completed => return Ok(()),
            OptimizationStatus::Stopped | OptimizationStatus::Failed => {
                // Already ended: just make sure nothing is left alive or unrevoked.
                let agents = self.ledger.list_agents(&oid(id)).await?;
                join_all(agents.iter().map(|a| self.revoke_agent(a))).await;
                self.terminate_all(id).await?;
                self.safe_meter(id).await;
                return Ok(());
            }
            _ => {}
        }
        self.execute_kill(
            id,
            reason.unwrap_or("stopped by user"),
            OptimizationStatus::Stopped,
        )
        .await
    }

    pub(crate) async fn execute_kill(
        &self,
        id: &str,
        reason: &str,
        final_status: OptimizationStatus,
    ) -> EngineResult<()> {
        let rt = self.state(id);
        rt.killing.store(true, Ordering::SeqCst);
        rt.abort.cancel();
        self.kv.set(&k::stop_mode(id), "kill").await?;
        self.kv.set(&k::stop_reason(id), reason).await?;
        let prev = self.require_opt(id).await?.status;
        if prev != OptimizationStatus::Stopping {
            self.set_status(id, OptimizationStatus::Stopping).await?;
        }

        let agents = self.ledger.list_agents(&oid(id)).await?;
        // 1. revoke every agent identity: nothing an agent already holds works after this.
        join_all(agents.iter().map(|a| self.revoke_agent(a))).await;
        // 2. ask the runners to stop.
        let runners = self.runners_for(id).await;
        join_all(
            agents
                .iter()
                .filter(|a| a.runner_instance_id.is_some() && a.status != AgentStatus::Stopped)
                .map(|a| {
                    let runners = runners.clone();
                    async move {
                        if let Err(e) = runners.stop(&handle(a)).await {
                            self.report_error(e, &format!("runners.stop {}", a.id));
                        }
                    }
                }),
        )
        .await;
        // 3. terminate everything tagged for this optimization.
        self.terminate_all(id).await?;
        for a in &agents {
            if let Some(cur) = self.ledger.get_agent(&a.id).await?
                && !terminal_agent_status(cur.status)
            {
                self.ledger
                    .set_agent_status(&a.id, AgentStatus::Stopped, Patch::Keep)
                    .await?;
            }
        }
        self.safe_meter(id).await;
        self.set_status(id, final_status).await?;
        rt.clear_job();
        rt.killing.store(false, Ordering::SeqCst);
        Ok(())
    }

    pub(crate) async fn revoke_agent(&self, agent: &ratchet_core::rows::Agent) {
        let r: EngineResult<()> = async {
            if self.kv.has(&k::revoked(agent.id.as_str())).await? {
                return Ok(());
            }
            let Some(identity) = self
                .kv
                .get_json::<AgentIdentity>(&k::identity(agent.id.as_str()))
                .await?
            else {
                return Ok(());
            };
            match self.ports.compute.revoke_agent_identity(&identity).await {
                Ok(()) => {
                    self.kv
                        .set(
                            &k::revoked(agent.id.as_str()),
                            &self.clock.now().to_string(),
                        )
                        .await?;
                }
                Err(err) => self.report_error(err, &format!("revoke {}", agent.id)), // retried by the reaper pass
            }
            Ok(())
        }
        .await;
        if let Err(e) = r {
            self.report_error(e, &format!("revoke {}", agent.id));
        }
    }

    pub(crate) async fn terminate_all(&self, id: &str) -> EngineResult<Vec<String>> {
        let live: Vec<String> = self
            .ports
            .compute
            .list_tagged(Some(&oid(id)))
            .await?
            .into_iter()
            .filter(|i| i.state != ratchet_core::ports::InstanceState::Terminated)
            .map(|i| i.id)
            .collect();
        if !live.is_empty() {
            self.ports.compute.terminate(&live).await?;
        }
        Ok(live)
    }

    // ------------------------------------------------------------------ agent control

    pub async fn pause_agent(&self, agent_id: &str) -> EngineResult<()> {
        let agent = self.live_agent(agent_id).await?;
        if agent.status == AgentStatus::Paused {
            return Ok(());
        }
        self.runners_for(agent.optimization_id.as_str())
            .await
            .send(&handle(&agent), &ratchet_core::ports::RunnerMessage::Pause)
            .await?;
        self.ledger
            .set_agent_status(&agent.id, AgentStatus::Paused, Patch::Keep)
            .await?;
        self.log_agent(
            &agent,
            AgentEvent::new(self.clock.now(), AgentEventType::Status)
                .with_text("paused by user")
                .with_data(serde_json::json!({ "state": "paused" })),
        )
        .await
    }

    pub async fn resume_agent(&self, agent_id: &str) -> EngineResult<()> {
        let agent = self.live_agent(agent_id).await?;
        if agent.status != AgentStatus::Paused {
            return Ok(());
        }
        self.runners_for(agent.optimization_id.as_str())
            .await
            .send(&handle(&agent), &ratchet_core::ports::RunnerMessage::Resume)
            .await?;
        self.ledger
            .set_agent_status(&agent.id, AgentStatus::Running, Patch::Keep)
            .await?;
        self.log_agent(
            &agent,
            AgentEvent::new(self.clock.now(), AgentEventType::Status)
                .with_text("resumed by user")
                .with_data(serde_json::json!({ "state": "running" })),
        )
        .await
    }

    pub async fn message_agent(&self, agent_id: &str, text: &str) -> EngineResult<()> {
        if text.trim().is_empty() {
            return Err(EngineError::invalid_input("message must not be empty"));
        }
        let agent = self.live_agent(agent_id).await?;
        self.runners_for(agent.optimization_id.as_str())
            .await
            .send(
                &handle(&agent),
                &ratchet_core::ports::RunnerMessage::User {
                    text: text.to_owned(),
                },
            )
            .await?;
        self.log_agent(
            &agent,
            AgentEvent::new(self.clock.now(), AgentEventType::Message)
                .with_text(text)
                .with_data(serde_json::json!({ "from": "user" })),
        )
        .await
    }

    async fn live_agent(&self, agent_id: &str) -> EngineResult<ratchet_core::rows::Agent> {
        assert_valid_id("agent id", agent_id)?;
        let agent = self
            .ledger
            .get_agent(&aid(agent_id))
            .await?
            .ok_or_else(|| EngineError::not_found(format!("agent {agent_id} not found")))?;
        let opt = self.require_opt(agent.optimization_id.as_str()).await?;
        if opt.status != OptimizationStatus::Running
            || agent.runner_instance_id.is_none()
            || !live_agent_status(agent.status)
        {
            return Err(EngineError::invalid_state(format!(
                "agent {agent_id} is not running (optimization {}, agent {})",
                opt.status, agent.status
            )));
        }
        Ok(agent)
    }

    // ------------------------------------------------------------------ calibration, protected paths

    /// Noise-floor options: the engine setting, with `evaluator.noise_factor` from ratchet.yaml taking precedence for k.
    pub(crate) fn noise_options(&self, config: &RatchetConfig) -> NoiseFloorOptions {
        match config.evaluator.noise_factor {
            None => self.settings.noise,
            Some(kk) => NoiseFloorOptions {
                k: Some(kk),
                ..self.settings.noise
            },
        }
    }

    /// The calibration of an optimization (repeats per candidate, win threshold), derived from the baseline in the
    /// ledger and remembered in the kv. Recomputed from the ledger when the kv entry is missing (a restart between the
    /// two writes), so it never disagrees with the baseline. `None` until the baseline exists.
    pub(crate) async fn calibration_for(
        &self,
        id: &str,
        config: &RatchetConfig,
    ) -> EngineResult<Option<Calibration>> {
        if let Some(stored) = self.kv.get_json::<Calibration>(&k::calibration(id)).await? {
            return Ok(Some(stored));
        }
        let Some(baseline) = self.ledger.get_baseline(&oid(id)).await? else {
            return Ok(None);
        };
        let c = calibrate_baseline(&ratchet_core::CalibrationInput {
            samples: &baseline.samples,
            repeats: config.evaluator.repeats,
            min_gain: config.objective.min_gain,
            noise_factor: self.noise_options(config).k,
        })
        .map_err(|e| EngineError::internal(e.to_string()))?;
        self.kv.set_json(&k::calibration(id), &c).await?;
        Ok(Some(c))
    }

    /// Runs per candidate / confirm: ratchet.yaml's `repeats`, else the calibrated number, else the baseline default.
    pub(crate) async fn repeats_for(&self, id: &str, config: &RatchetConfig) -> EngineResult<u64> {
        if let Some(r) = config.evaluator.repeats {
            return Ok(r);
        }
        Ok(self
            .calibration_for(id, config)
            .await?
            .map_or(cal::BASELINE_REPEATS, |c| c.repeats))
    }

    pub async fn calibration(&self, id: &str) -> EngineResult<Option<Calibration>> {
        assert_valid_id("optimization id", id)?;
        self.ready().await;
        self.require_opt(id).await?;
        let config = self.config(id).await?;
        self.calibration_for(id, &config).await
    }

    pub async fn protected_paths(&self, id: &str) -> EngineResult<EffectiveProtected> {
        assert_valid_id("optimization id", id)?;
        self.ready().await;
        self.require_opt(id).await?;
        let config = self.config(id).await?;
        Ok(effective_protected(
            &ProtectedConfigInput::from(&*config),
            None,
        ))
    }

    // ------------------------------------------------------------------ dynamic environments

    /// The environment this optimization is measured in (docs/SIMPLIFIED_MODEL.md, "Dynamic environments"), read from a
    /// checkout of the PROTECTED ref pinned to one commit for the life of the optimization (`base_sha`): the approved
    /// drafted environment under `infra/ratchet/` when `environment.scope` says so, or the custom module `infra:`
    /// names. `None` for scope `none` (and `single` without OpenTofu). Candidates cannot change it: their copy is
    /// never read, and the paths are protected so a candidate touching them is blocked.
    pub(crate) async fn environment_of(
        &self,
        id: &str,
    ) -> EngineResult<Option<SandboxEnvironment>> {
        let opt = self.require_opt(id).await?;
        let config = self.config(id).await?;
        if ratchet_env::environment_module_of(&(&*config).into()).is_none() {
            return Ok(None);
        }
        let git = self.git_for(&opt.repo);
        let commit = self.base_sha(&opt).await?;
        let root = self.scratch_root();
        tokio::fs::create_dir_all(&root)
            .await
            .map_err(|e| EngineError::internal(e.to_string()))?;
        let dir = tempfile::Builder::new()
            .prefix("ratchet-env-")
            .tempdir_in(&root)
            .map_err(|e| EngineError::internal(e.to_string()))?;
        git.checkout(&commit, dir.path()).await?;
        let path = dir.path().to_path_buf();
        let base_ref = opt.base_ref.clone();
        let config = config.clone();
        tokio::task::spawn_blocking(move || {
            ratchet_env::read_environment(&path, &config, &commit, &base_ref)
        })
        .await
        .map_err(|e| EngineError::internal(e.to_string()))?
        .map_err(Into::into)
    }

    /// The environment drafter (the setup agent), or `unavailable` when the engine has none.
    pub fn environment_drafter(&self) -> EngineResult<Arc<dyn ratchet_env::EnvironmentDrafter>> {
        self.ports.environment.clone().ok_or_else(|| {
            EngineError::new(
                EngineErrorCode::Unavailable,
                "this engine has no environment drafter (no setup agent is configured)",
            )
        })
    }

    pub async fn draft_environment(
        &self,
        input: ratchet_env::DraftEnvironmentInput,
    ) -> EngineResult<ratchet_env::EnvironmentDraftRecord> {
        self.ready().await;
        Ok(self.environment_drafter()?.draft(input).await?)
    }

    pub async fn revise_environment(
        &self,
        draft_id: &str,
        message: &str,
    ) -> EngineResult<ratchet_env::EnvironmentDraftRecord> {
        self.ready().await;
        Ok(self.environment_drafter()?.revise(draft_id, message).await?)
    }

    pub async fn approve_environment(
        &self,
        draft_id: &str,
        options: ratchet_env::ApproveEnvironmentOptions,
    ) -> EngineResult<ratchet_env::EnvironmentDraftRecord> {
        self.ready().await;
        Ok(self.environment_drafter()?.approve(draft_id, options).await?)
    }

    pub fn environment_draft(
        &self,
        draft_id: &str,
    ) -> EngineResult<Option<ratchet_env::EnvironmentDraftRecord>> {
        Ok(self.environment_drafter()?.get(draft_id))
    }

    pub fn discard_environment_draft(&self, draft_id: &str) -> EngineResult<()> {
        Ok(self.environment_drafter()?.discard(draft_id)?)
    }

    // ------------------------------------------------------------------ helpers

    pub(crate) fn state(&self, id: &str) -> Arc<Runtime> {
        lock(&self.rt)
            .entry(id.to_owned())
            .or_insert_with(|| Arc::new(Runtime::new()))
            .clone()
    }

    pub(crate) async fn require_opt(
        &self,
        id: &str,
    ) -> EngineResult<ratchet_core::rows::Optimization> {
        self.ledger
            .get_optimization(&oid(id))
            .await?
            .ok_or_else(|| EngineError::not_found(format!("optimization {id} not found")))
    }

    pub(crate) async fn require_agent(&self, id: &str) -> EngineResult<ratchet_core::rows::Agent> {
        self.ledger
            .get_agent(&aid(id))
            .await?
            .ok_or_else(|| EngineError::not_found(format!("agent {id} not found")))
    }

    pub(crate) async fn config(&self, id: &str) -> EngineResult<Arc<RatchetConfig>> {
        if let Some(c) = lock(&self.configs).get(id) {
            return Ok(c.clone());
        }
        let opt = self.require_opt(id).await?;
        match parse_config(&opt.config_yaml) {
            Ok(c) => {
                let c = Arc::new(c);
                lock(&self.configs).insert(id.to_owned(), c.clone());
                Ok(c)
            }
            Err(errors) => Err(EngineError::new(
                EngineErrorCode::InvalidConfig,
                format!(
                    "stored config for {id} is invalid: {}",
                    errors
                        .iter()
                        .map(format_config_error)
                        .collect::<Vec<_>>()
                        .join("; ")
                ),
            )),
        }
    }

    pub(crate) async fn set_status(
        &self,
        id: &str,
        status: OptimizationStatus,
    ) -> EngineResult<()> {
        self.ledger
            .set_optimization_status(&oid(id), status, Some(self.clock.now()))
            .await?;
        self.emit(EngineEvent::Optimization {
            optimization_id: oid(id),
            status: status.to_string(),
        });
        self.sync_run_active().await;
        Ok(())
    }

    /// Optimizations that hold the machine awake: starting, baselining, running or stopping.
    async fn active_run_ids(&self) -> Vec<String> {
        let mut ids: Vec<String> = lock(&self.pending_starts).iter().cloned().collect();
        ids.sort();
        let statuses = [
            OptimizationStatus::Baselining,
            OptimizationStatus::Running,
            OptimizationStatus::Stopping,
        ];
        if let Ok(list) = self.ledger.list_optimizations(Some(&statuses)).await {
            for o in list {
                let id = o.id.to_string();
                if !ids.contains(&id) {
                    ids.push(id);
                }
            }
        }
        ids
    }

    /// Emit `RunActive` when the "any run is live" signal flips (the app turns it into a prevent-sleep assertion).
    pub(crate) async fn sync_run_active(&self) {
        let ids = self.active_run_ids().await;
        let active = !ids.is_empty();
        if self.run_active.swap(active, Ordering::SeqCst) == active {
            return;
        }
        self.emit(EngineEvent::RunActive {
            active,
            optimization_ids: ids.iter().map(|i| oid(i)).collect(),
        });
    }

    pub(crate) async fn log_agent(
        &self,
        agent: &ratchet_core::rows::Agent,
        e: AgentEvent,
    ) -> EngineResult<()> {
        self.ledger
            .append_agent_event(ratchet_ledger::AppendAgentEventInput {
                agent_id: agent.id.clone(),
                event_type: event_type_str(e.event_type),
                payload: Some(serde_json::to_value(&e)?),
                at: Some(e.at),
            })
            .await?;
        self.emit(EngineEvent::AgentEvent {
            optimization_id: agent.optimization_id.clone(),
            agent_id: agent.id.clone(),
            event: e,
        });
        Ok(())
    }

    pub(crate) async fn safe_meter(&self, id: &str) {
        if let Err(e) = self.meter.meter(id, None).await {
            self.report_error(e, &format!("meter {id}"));
        }
    }
}

pub(crate) fn handle(agent: &ratchet_core::rows::Agent) -> RunnerHandle {
    RunnerHandle {
        optimization_id: agent.optimization_id.clone(),
        agent_id: agent.id.clone(),
        instance_id: agent.runner_instance_id.clone().unwrap_or_default(),
    }
}

/// `path.normalize` for a repo-relative path: `None` when it is absolute or climbs out of the repository.
pub(crate) fn normalize_inside(rel: &str) -> Option<PathBuf> {
    let p = Path::new(rel);
    if p.is_absolute() {
        return None;
    }
    let mut out = PathBuf::new();
    for c in p.components() {
        match c {
            Component::Normal(s) => out.push(s),
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    return None;
                }
            }
            _ => return None,
        }
    }
    Some(out)
}

async fn read_inside(dir: &Path, rel: &str) -> Result<String, String> {
    let n = normalize_inside(rel).ok_or_else(|| format!("path escapes the repository: {rel}"))?;
    tokio::fs::read_to_string(dir.join(n))
        .await
        .map_err(|e| e.to_string())
}

// ---------------------------------------------------------------------------------------- EngineApi

fn port(e: EngineError) -> PortError {
    e.into()
}

#[async_trait]
impl EngineApi for Engine {
    fn mode(&self) -> Mode {
        self.mode
    }

    async fn create_optimization(
        &self,
        input: &EngineCreateOptimizationInput,
    ) -> PortResult<OptimizationId> {
        Engine::create_optimization(self, input.into())
            .await
            .map(|i| oid(&i))
            .map_err(port)
    }

    async fn start(&self, id: &OptimizationId) -> PortResult<()> {
        Engine::start(self, id.as_str()).await.map_err(port)
    }

    async fn stop(&self, id: &OptimizationId, reason: Option<&str>) -> PortResult<()> {
        Engine::stop(self, id.as_str(), reason).await.map_err(port)
    }

    async fn pause_agent(&self, agent_id: &AgentId) -> PortResult<()> {
        Engine::pause_agent(self, agent_id.as_str())
            .await
            .map_err(port)
    }

    async fn resume_agent(&self, agent_id: &AgentId) -> PortResult<()> {
        Engine::resume_agent(self, agent_id.as_str())
            .await
            .map_err(port)
    }

    async fn message_agent(&self, agent_id: &AgentId, text: &str) -> PortResult<()> {
        Engine::message_agent(self, agent_id.as_str(), text)
            .await
            .map_err(port)
    }

    async fn open_pull_request(
        &self,
        id: &OptimizationId,
    ) -> PortResult<ratchet_core::ports::PullRequestRef> {
        Engine::open_pull_request(self, id.as_str())
            .await
            .map_err(port)
    }

    async fn teardown(&self, id: &OptimizationId) -> PortResult<()> {
        Engine::teardown(self, id.as_str()).await.map_err(port)
    }

    async fn reconcile_costs(
        &self,
        id: &OptimizationId,
    ) -> PortResult<ratchet_core::ports::CostReconcileReport> {
        Engine::reconcile_costs(self, id.as_str())
            .await
            .map_err(port)
    }

    async fn calibration(&self, id: &OptimizationId) -> PortResult<Option<Calibration>> {
        Engine::calibration(self, id.as_str()).await.map_err(port)
    }

    async fn protected_paths(&self, id: &OptimizationId) -> PortResult<EffectiveProtected> {
        Engine::protected_paths(self, id.as_str())
            .await
            .map_err(port)
    }

    async fn propose_template(
        &self,
        input: &ratchet_core::ports::ProposeTemplateInput,
    ) -> PortResult<ratchet_core::ports::ProposeTemplateResult> {
        Engine::propose_template(self, input).await.map_err(port)
    }

    async fn sandbox_state(&self, sandbox_id: &str) -> PortResult<SandboxState> {
        Engine::sandbox_state(self, sandbox_id).await.map_err(port)
    }

    async fn plan_sandbox(&self, input: &SandboxInput) -> PortResult<SandboxPlan> {
        Engine::plan_sandbox(self, input).await.map_err(port)
    }

    async fn apply_sandbox(&self, input: &SandboxInput) -> PortResult<SandboxOutputs> {
        Engine::apply_sandbox(self, input).await.map_err(port)
    }

    async fn destroy_sandbox(&self, sandbox_id: &str) -> PortResult<()> {
        Engine::destroy_sandbox(self, sandbox_id)
            .await
            .map_err(port)
    }

    fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        Engine::subscribe(self)
    }

    async fn teardown_receipt(
        &self,
        optimization_id: &OptimizationId,
    ) -> PortResult<Option<ratchet_core::ports::SandboxTeardownReceipt>> {
        Engine::teardown_receipt(self, optimization_id.as_str())
            .await
            .map_err(port)
    }

    async fn sandbox_activity(
        &self,
        sandbox_id: &str,
    ) -> PortResult<ratchet_core::ports::SandboxActivity> {
        Engine::sandbox_activity(self, sandbox_id)
            .await
            .map_err(port)
    }

    async fn end_sandbox(
        &self,
        sandbox_id: &str,
    ) -> PortResult<ratchet_core::ports::SandboxTeardownReceipt> {
        Engine::end_sandbox(self, sandbox_id).await.map_err(port)
    }

    fn run_active(&self) -> bool {
        Engine::run_active(self)
    }
}
