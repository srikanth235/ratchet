//! The simulation service: an `AppService` backed by `ratchet-ledger`, `ratchet_core::ports::mock` and seeded demo
//! data, so the Mac app (and the CLI) can be built against a working daemon before the real engine lands.
//!
//! - the ledger is real (in memory, or a file with `--ledger`), so every read goes through the same code the
//!   real engine will use ([`crate::views::LedgerViews`]);
//! - runs are scripted ([`engine`]): baselines, agent turns, verdicts through the real ratchet rule, spend,
//!   events; seeded optimizations are resumed at boot;
//! - everything outside the ledger (settings, GitHub sign-in, connectors, environment drafts, sandboxes) is in
//!   memory ([`settings`], [`connectors`], [`drafts`], [`environments`]).

mod connectors;
mod data;
mod drafts;
mod engine;
mod environments;
mod overview;
mod seed;
mod settings;

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use ratchet_core::config::RatchetConfig;
use ratchet_core::enums::{AgentStatus, Mode, OptimizationStatus, TemplateKind};
use ratchet_core::ids::{AgentId, OptimizationId};
use ratchet_core::ledger::Patch;
use ratchet_core::ports::mock::{MockCostReconciler, SystemClock};
use ratchet_core::ports::{
    Clock, CostReconcileReport, ProposeTemplateInput, ProposeTemplateResult,
};
use ratchet_core::templates::TemplateInfo;
use ratchet_core::{Calibration, EffectiveProtected, ProtectedConfigInput, effective_protected};
use ratchet_ledger::{
    AddAgentInput, CreateOptimizationInput as LedgerCreate, Ledger, LedgerOptions, ValueInput,
};
use tokio::sync::broadcast;
use tokio::task::JoinHandle;

pub use data::{DEFAULT_CONFIG_YAML, Rng};
pub use seed::DEMO_RUNNING_ID;

use self::connectors::ConnectorsState;
use self::drafts::DraftsState;
use self::environments::EnvState;
use self::settings::SettingsState;
use crate::api::*;
use crate::service::{AppService, ServiceError, ServiceResult};
use crate::views::{LedgerViews, PrUrlLookup};

/// How the simulation behaves.
#[derive(Clone)]
pub struct SimOptions {
    /// The ledger file; `None` = in memory (seeded again at every start).
    pub ledger_path: Option<PathBuf>,
    /// Seed the demo optimizations when the ledger is empty.
    pub seed_demo: bool,
    /// Drive runs with timers. Off: runs advance only through [`SimService::tick`] (tests).
    pub auto_run: bool,
    /// Real-time ms between simulated agent turns.
    pub tick_ms: u64,
    /// Real-time ms before a simulated baseline completes.
    pub baseline_ms: u64,
    /// PRNG seed.
    pub rng_seed: u32,
    /// Time source; default the system clock.
    pub clock: Option<Arc<dyn Clock>>,
    /// Idle window before an unused environment is removed, minutes.
    pub idle_minutes: u32,
    /// Resume seeded / stored runs that were baselining or running.
    pub resume_runs: bool,
    /// Real-time ms a simulated GitHub device flow takes (0 = never completes by itself).
    pub github_flow_ms: u64,
}

impl Default for SimOptions {
    fn default() -> Self {
        Self {
            ledger_path: None,
            seed_demo: true,
            auto_run: true,
            tick_ms: 2500,
            baseline_ms: 1200,
            rng_seed: 0x5eed,
            clock: None,
            idle_minutes: 30,
            resume_runs: true,
            github_flow_ms: 2000,
        }
    }
}

pub(crate) struct Run {
    pub config: RatchetConfig,
    pub no_eval: u32,
    pub turn: u32,
    pub task: Option<JoinHandle<()>>,
}

#[derive(Default)]
pub(crate) struct State {
    pub runs: HashMap<String, Run>,
    pub plateau: HashMap<String, u32>,
    pub inbox: HashMap<String, VecDeque<String>>,
    pub last_active: Vec<String>,
    pub settings: SettingsState,
    pub env: EnvState,
    pub connectors: ConnectorsState,
    pub drafts: DraftsState,
}

pub(crate) struct Inner {
    pub ledger: Ledger,
    pub views: LedgerViews,
    pub tx: broadcast::Sender<ApiEvent>,
    pub clock: Arc<dyn Clock>,
    pub opts: SimOptions,
    pub rng: Mutex<Rng>,
    pub pr_urls: Arc<Mutex<HashMap<String, String>>>,
    pub state: Mutex<State>,
}

impl Inner {
    pub(crate) fn st(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub(crate) fn rng(&self) -> MutexGuard<'_, Rng> {
        self.rng.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Publish an event (nobody listening is fine).
    pub(crate) fn emit(&self, e: ApiEvent) {
        let _ = self.tx.send(e);
    }

    /// Publish `run-active` when the set of active optimizations changed.
    pub(crate) async fn publish_run_active(&self) {
        let Ok(rows) = self.ledger.list_optimizations(None).await else {
            return;
        };
        let mut ids: Vec<String> = rows
            .iter()
            .filter(|o| o.status.is_active())
            .map(|o| o.id.to_string())
            .collect();
        ids.sort();
        let changed = {
            let mut st = self.st();
            if st.last_active == ids {
                false
            } else {
                st.last_active.clone_from(&ids);
                true
            }
        };
        if changed {
            self.emit(ApiEvent::RunActive {
                active: !ids.is_empty(),
                optimization_ids: ids,
            });
        }
    }

    pub(crate) fn engine(&self) -> EngineInfo {
        EngineInfo {
            kind: EngineKind::Sim,
            mode: Mode::Simulation,
        }
    }
}

/// Ledger columns that come from ratchet.yaml (template category, objective name, evaluator kind, value model),
/// as the core engine sets them, with the rest of a new optimization.
pub(crate) fn new_optimization_input(
    repo: &str,
    base_ref: &str,
    cfg: &RatchetConfig,
    yaml: &str,
    budget_usd: f64,
    time_limit_s: i64,
    name: Option<String>,
) -> LedgerCreate {
    let mut input = LedgerCreate::new(
        repo,
        base_ref,
        cfg.objective.metric.clone(),
        cfg.objective.direction,
        cfg.objective
            .min_gain
            .unwrap_or(ratchet_core::bench::calibrate::calibration::FALLBACK_GAIN),
        budget_usd,
        time_limit_s,
        yaml,
    );
    input.name = name.or_else(|| cfg.objective.name.clone());
    input.pack = cfg.pack.clone();
    input.objective_name = cfg.objective.name.clone();
    input.evaluator_kind = Some(cfg.evaluator.kind);
    input.value = cfg.objective.value.as_ref().map(|v| ValueInput {
        usd_per_unit: v.usd_per_unit,
        period: v.period,
        basis: v.basis,
        unit: v.unit.clone(),
    });
    input
}

/// `https://github.com/acme/search-api.git` / `git@github.com:acme/search-api` / `acme/search-api` -> `acme/search-api`.
pub fn repo_slug(repo_url: &str) -> Option<String> {
    let s = repo_url.trim().trim_end_matches('/');
    let s = s.strip_suffix(".git").unwrap_or(s);
    let tail = s.rsplit([':', '/']).take(2).collect::<Vec<_>>();
    if tail.len() < 2 {
        return None;
    }
    let (name, owner) = (tail[0], tail[1]);
    let ok = |p: &str| {
        !p.is_empty()
            && p.chars()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-'))
    };
    (ok(owner) && ok(name)).then(|| format!("{owner}/{name}"))
}

/// The simulation, as an `AppService`.
#[derive(Clone)]
pub struct SimService(pub(crate) Arc<Inner>);

impl SimService {
    pub async fn new(opts: SimOptions) -> ServiceResult<Self> {
        let clock: Arc<dyn Clock> = opts.clock.clone().unwrap_or_else(|| Arc::new(SystemClock));
        let ledger_opts = LedgerOptions {
            clock: Some(Arc::clone(&clock)),
            max_connections: None,
        };
        let ledger = match &opts.ledger_path {
            Some(p) => {
                if let Some(dir) = p.parent() {
                    tokio::fs::create_dir_all(dir).await.map_err(|e| {
                        ServiceError::Internal(format!("cannot create {}: {e}", dir.display()))
                    })?;
                }
                Ledger::open(p, ledger_opts).await?
            }
            None => Ledger::open_memory(ledger_opts).await?,
        };
        let pr_urls: Arc<Mutex<HashMap<String, String>>> = Arc::default();
        let lookup: PrUrlLookup = {
            let m = Arc::clone(&pr_urls);
            Arc::new(move |id| {
                m.lock()
                    .unwrap_or_else(PoisonError::into_inner)
                    .get(id)
                    .cloned()
            })
        };
        let (tx, _) = broadcast::channel(1024);
        let views = LedgerViews::new(ledger.clone(), Arc::clone(&clock), lookup);
        let inner = Arc::new(Inner {
            ledger,
            views,
            tx,
            clock: Arc::clone(&clock),
            rng: Mutex::new(Rng::new(opts.rng_seed)),
            pr_urls,
            state: Mutex::new(State::default()),
            opts: opts.clone(),
        });
        inner.st().settings = SettingsState::demo();
        inner.st().connectors = ConnectorsState::demo();
        if opts.seed_demo && seed::is_ledger_empty(&inner.ledger).await? {
            seed::seed_demo(&inner.ledger, clock.now()).await?;
            inner.seed_environment().await?;
        }
        if opts.resume_runs {
            let live = inner
                .ledger
                .list_optimizations(Some(&[
                    OptimizationStatus::Baselining,
                    OptimizationStatus::Running,
                ]))
                .await?;
            for o in live {
                inner.start(o.id.as_str()).await?;
            }
        }
        Ok(Self(inner))
    }

    /// An in-memory simulation that only moves when told to (tests).
    pub async fn manual() -> ServiceResult<Self> {
        Self::new(SimOptions {
            auto_run: false,
            github_flow_ms: 0,
            ..SimOptions::default()
        })
        .await
    }

    /// Run one simulated agent turn for every running agent of `id` (what the timer does).
    pub async fn tick(&self, id: &str) -> ServiceResult<()> {
        self.0.tick(id).await
    }

    /// Finish a pending simulated GitHub device flow (signed in as the demo account).
    pub fn complete_github_flow(&self) {
        self.0.complete_github_flow();
    }

    /// The ledger the simulation writes to (tests, `ratchetd` diagnostics).
    pub fn ledger(&self) -> &Ledger {
        &self.0.ledger
    }

    /// Stop every timer. Ledger state is untouched.
    pub fn shutdown(&self) {
        self.0.halt_all();
    }
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.halt_all();
    }
}

fn effective_protected_for(cfg: &RatchetConfig) -> EffectiveProtected {
    effective_protected(&ProtectedConfigInput::from(cfg), None)
}

#[async_trait]
impl AppService for SimService {
    fn engine(&self) -> EngineInfo {
        self.0.engine()
    }

    fn ledger_path(&self) -> Option<String> {
        self.0
            .opts
            .ledger_path
            .as_ref()
            .map(|p| p.display().to_string())
    }

    fn subscribe(&self) -> broadcast::Receiver<ApiEvent> {
        self.0.tx.subscribe()
    }

    // ---- optimizations -----------------------------------------------------------------------------------

    async fn list_optimizations(&self, range: KpiRange) -> ServiceResult<OptimizationsSnapshot> {
        self.0.views.snapshot(range, self.0.engine()).await
    }

    async fn create_optimization(
        &self,
        input: CreateOptimizationInput,
    ) -> ServiceResult<CreatedOptimization> {
        self.0.create_optimization(input).await
    }

    async fn get_optimization(&self, id: &str) -> ServiceResult<OptimizationDetail> {
        let mut d = self.0.views.detail(id).await?;
        d.receipt = self.receipt(id).await?;
        d.calibration = self.calibration(id).await?.map(|c| c.explanation);
        Ok(d)
    }

    async fn start_optimization(&self, id: &str) -> ServiceResult<()> {
        self.0.start(id).await
    }

    async fn stop_optimization(&self, id: &str, _reason: Option<String>) -> ServiceResult<()> {
        self.0.stop(id).await
    }

    async fn ratchet_history(
        &self,
        id: &str,
        metric: Option<String>,
    ) -> ServiceResult<RatchetSeries> {
        self.0.views.series(id, metric.as_deref()).await
    }

    async fn winning_change(&self, id: &str) -> ServiceResult<WinningChangeVm> {
        // The simulation has no git behind it: say so instead of inventing a diff.
        self.0
            .views
            .winning_change(
                id,
                None,
                Some("This engine has no git access to diff the winning commit.".into()),
            )
            .await
    }

    async fn candidates(&self, id: &str) -> ServiceResult<Vec<CandidateVm>> {
        self.0.views.candidates(id).await
    }

    async fn spend(&self, id: &str) -> ServiceResult<SpendVm> {
        self.0.views.spend(id).await
    }

    async fn reconcile_costs(&self, id: &str) -> ServiceResult<CostReconcileReport> {
        let oid = OptimizationId::new(id);
        // "Billing" is the live estimate plus 4% (the estimates only, so a second run finds nothing to correct).
        let billed: Vec<_> = self
            .0
            .views
            .estimated_by_category(id)
            .await?
            .into_iter()
            .filter(|(c, _)| *c != ratchet_core::SpendCategory::Tokens)
            .map(|(c, usd)| (c, (usd * 1.04 * 10_000.0).round() / 10_000.0))
            .filter(|(_, usd)| *usd > 0.0)
            .collect();
        let reconciler = MockCostReconciler::complete(&billed);
        ratchet_core::reconcile_spend(
            &self.0.ledger,
            &reconciler,
            &oid,
            ratchet_core::ReconcileSpendOptions::at(self.0.clock.now()),
        )
        .await
        .map_err(|e| match e {
            ratchet_core::cost::ReconcileError::NotFound(_) => {
                ServiceError::not_found("optimization", id)
            }
            ratchet_core::cost::ReconcileError::Ledger(l) => l.into(),
            ratchet_core::cost::ReconcileError::Billing(p) => p.into(),
        })
    }

    async fn calibration(&self, id: &str) -> ServiceResult<Option<Calibration>> {
        let oid = OptimizationId::new(id);
        let Some(opt) = self.0.ledger.get_optimization(&oid).await? else {
            return Err(ServiceError::not_found("optimization", id));
        };
        let Some(base) = self.0.ledger.get_baseline(&oid).await? else {
            return Ok(None);
        };
        let cfg = ratchet_core::parse_config(&opt.config_yaml).ok();
        let c = ratchet_core::bench::calibrate::calibrate(&ratchet_core::CalibrationInput {
            samples: &base.samples,
            repeats: cfg.as_ref().and_then(|c| c.evaluator.repeats),
            min_gain: cfg.as_ref().and_then(|c| c.objective.min_gain),
            noise_factor: cfg.as_ref().and_then(|c| c.evaluator.noise_factor),
        })
        .map_err(|e| ServiceError::Internal(e.to_string()))?;
        Ok(Some(c))
    }

    async fn protected_paths(&self, id: &str) -> ServiceResult<EffectiveProtected> {
        let oid = OptimizationId::new(id);
        let opt = self
            .0
            .ledger
            .get_optimization(&oid)
            .await?
            .ok_or_else(|| ServiceError::not_found("optimization", id))?;
        let cfg = ratchet_core::parse_config(&opt.config_yaml)
            .map_err(|_| ServiceError::Internal("stored ratchet.yaml is invalid".into()))?;
        Ok(effective_protected_for(&cfg))
    }

    async fn receipt(&self, id: &str) -> ServiceResult<Option<TeardownReceiptVm>> {
        self.0.receipt_for(id).await
    }

    async fn open_pull_request(&self, id: &str) -> ServiceResult<PullRequestVm> {
        let oid = OptimizationId::new(id);
        let opt = self
            .0
            .ledger
            .get_optimization(&oid)
            .await?
            .ok_or_else(|| ServiceError::not_found("optimization", id))?;
        if self.0.ledger.ratchet_history(&oid).await?.is_empty() {
            return Err(ServiceError::Conflict(
                "no verified wins to open a pull request for".into(),
            ));
        }
        let n = 100 + u32::from(id.bytes().last().unwrap_or(0)) % 800;
        let url = format!("https://github.com/{}/pull/{n}", opt.repo);
        self.0
            .pr_urls
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(id.to_owned(), url.clone());
        Ok(PullRequestVm { url })
    }

    async fn report(&self, id: &str) -> ServiceResult<ReportVm> {
        self.0.views.report(id).await
    }

    async fn recent_activity(
        &self,
        optimization_id: Option<String>,
        limit: u32,
    ) -> ServiceResult<Vec<ActivityVm>> {
        self.0
            .views
            .activity(limit.clamp(1, 200) as usize, optimization_id.as_deref())
            .await
    }

    // ---- agents ------------------------------------------------------------------------------------------

    async fn agent_events(
        &self,
        agent_id: &str,
        cursor: Option<i64>,
        limit: u32,
    ) -> ServiceResult<AgentEventsPage> {
        self.0.views.agent_events(agent_id, cursor, limit).await
    }

    async fn pause_agent(&self, agent_id: &str) -> ServiceResult<()> {
        let aid = self.0.require_agent(agent_id).await?;
        self.0
            .ledger
            .set_agent_status(&aid, AgentStatus::Paused, Patch::Keep)
            .await?;
        self.0
            .log_event(&aid, "status", "Paused by user", None)
            .await
    }

    async fn resume_agent(&self, agent_id: &str) -> ServiceResult<()> {
        let aid = self.0.require_agent(agent_id).await?;
        self.0
            .ledger
            .set_agent_status(&aid, AgentStatus::Running, Patch::Keep)
            .await?;
        self.0.log_event(&aid, "status", "Resumed", None).await
    }

    async fn message_agent(&self, agent_id: &str, text: &str) -> ServiceResult<()> {
        let text = text.trim();
        if text.is_empty() || text.chars().count() > 4000 {
            return Err(ServiceError::InvalidFields {
                message: "the message must be 1 to 4000 characters".into(),
                fields: vec![FieldError::new("text", "must be 1 to 4000 characters")],
            });
        }
        let aid = self.0.require_agent(agent_id).await?;
        self.0
            .st()
            .inbox
            .entry(agent_id.to_owned())
            .or_default()
            .push_back(text.to_owned());
        self.0
            .log_event(
                &aid,
                "message",
                &format!("You: {text}"),
                Some(serde_json::json!({"from": "user"})),
            )
            .await
    }

    // ---- config and preflight ----------------------------------------------------------------------------

    async fn preflight(&self, input: PreflightInput) -> ServiceResult<Vec<PreflightItem>> {
        Ok(self.0.preflight(input))
    }

    // ---- settings ----------------------------------------------------------------------------------------

    async fn settings(&self) -> ServiceResult<SettingsVm> {
        Ok(self.0.settings_vm())
    }

    async fn set_aws_profile(&self, profile: &str) -> ServiceResult<SettingsVm> {
        self.0.set_aws_profile(profile)
    }

    async fn save_advanced_settings(&self, input: LiveSettingsInput) -> ServiceResult<SettingsVm> {
        self.0.save_advanced(input)
    }

    async fn set_secret(&self, name: SecretName, value: &str) -> ServiceResult<SettingsVm> {
        self.0.set_secret(name, value)
    }

    async fn clear_secret(&self, name: SecretName) -> ServiceResult<SettingsVm> {
        self.0.clear_secret(name)
    }

    async fn test_connection(&self, target: ConnectionTarget) -> ServiceResult<ConnectionResultVm> {
        Ok(self.0.test_connection(target))
    }

    // ---- GitHub, repos -----------------------------------------------------------------------------------

    async fn github_status(&self) -> ServiceResult<GithubStatusVm> {
        Ok(self.0.github_status())
    }

    async fn github_sign_in(&self) -> ServiceResult<GithubStatusVm> {
        Ok(self.0.github_sign_in())
    }

    async fn github_sign_out(&self) -> ServiceResult<GithubStatusVm> {
        Ok(self.0.github_sign_out())
    }

    async fn list_repos(&self, query: Option<String>) -> ServiceResult<Vec<RepoVm>> {
        self.0.list_repos(query.as_deref())
    }

    async fn inspect_repo(&self, source: RepoSource) -> ServiceResult<RepoInspectVm> {
        self.0.inspect_repo(source).await
    }

    // ---- AWS, agent CLIs ---------------------------------------------------------------------------------

    async fn aws_profiles(&self) -> ServiceResult<AwsProfilesVm> {
        Ok(self.0.aws_profiles())
    }

    async fn harnesses(&self, _refresh: bool) -> ServiceResult<Vec<HarnessVm>> {
        Ok(self.0.harnesses())
    }

    // ---- templates ---------------------------------------------------------------------------------------

    async fn list_templates(&self, kind: Option<TemplateKind>) -> ServiceResult<Vec<TemplateInfo>> {
        Ok(ratchet_core::list_templates(
            ratchet_core::templates::TemplateFilter {
                kind,
                category: None,
            },
        ))
    }

    async fn propose_template(
        &self,
        input: ProposeTemplateInput,
    ) -> ServiceResult<ProposeTemplateResult> {
        self.0.propose_template(input)
    }

    // ---- environments ------------------------------------------------------------------------------------

    async fn environments(&self) -> ServiceResult<EnvironmentsVm> {
        self.0.environments().await
    }

    async fn end_environment(&self, sandbox_id: &str) -> ServiceResult<EnvironmentsVm> {
        self.0.end_environment(sandbox_id).await?;
        self.0.environments().await
    }

    // ---- overview (provisional) --------------------------------------------------------------------------

    async fn overview(&self, range: OverviewRange) -> ServiceResult<OverviewVm> {
        self.0.overview(range).await
    }

    // ---- connectors (provisional) ------------------------------------------------------------------------

    async fn list_connectors(&self) -> ServiceResult<Vec<ConnectorVm>> {
        Ok(self.0.list_connectors())
    }

    async fn create_connector(&self, input: CreateConnectorInput) -> ServiceResult<ConnectorVm> {
        self.0.create_connector(input)
    }

    async fn get_connector(&self, id: &str) -> ServiceResult<ConnectorVm> {
        self.0.get_connector(id)
    }

    async fn update_connector(
        &self,
        id: &str,
        input: UpdateConnectorInput,
    ) -> ServiceResult<ConnectorVm> {
        self.0.update_connector(id, input)
    }

    async fn delete_connector(&self, id: &str) -> ServiceResult<()> {
        self.0.delete_connector(id)
    }

    async fn test_connector(&self, id: &str) -> ServiceResult<ConnectorTestVm> {
        self.0.test_connector(id)
    }

    async fn connector_selection(&self, repo: &str) -> ServiceResult<ConnectorSelectionVm> {
        self.0.connector_selection(repo)
    }

    async fn save_connector_selection(
        &self,
        input: SaveConnectorSelectionInput,
    ) -> ServiceResult<ConnectorSelectionVm> {
        self.0.save_connector_selection(input)
    }

    // ---- environment drafts (provisional) ----------------------------------------------------------------

    async fn create_environment_draft(
        &self,
        input: CreateEnvironmentDraftInput,
    ) -> ServiceResult<EnvironmentDraftVm> {
        self.0.create_draft(input)
    }

    async fn get_environment_draft(&self, id: &str) -> ServiceResult<EnvironmentDraftVm> {
        self.0.get_draft(id)
    }

    async fn environment_draft_files(&self, id: &str) -> ServiceResult<EnvFilesVm> {
        self.0.draft_files(id)
    }

    async fn revise_environment_draft(
        &self,
        id: &str,
        input: ReviseEnvironmentDraftInput,
    ) -> ServiceResult<EnvironmentDraftVm> {
        self.0.revise_draft(id, input)
    }

    async fn approve_environment_draft(&self, id: &str) -> ServiceResult<EnvironmentDraftVm> {
        self.0.approve_draft(id)
    }

    async fn open_environment_draft_pr(&self, id: &str) -> ServiceResult<EnvironmentDraftVm> {
        self.0.open_draft_pr(id)
    }
}

impl Inner {
    async fn require_agent(&self, id: &str) -> ServiceResult<AgentId> {
        let aid = AgentId::new(id);
        self.ledger
            .get_agent(&aid)
            .await?
            .ok_or_else(|| ServiceError::not_found("agent", id))?;
        Ok(aid)
    }

    async fn create_optimization(
        &self,
        input: CreateOptimizationInput,
    ) -> ServiceResult<CreatedOptimization> {
        let mut fields = Vec::new();
        if input.agents.is_empty() {
            fields.push(FieldError::new("agents", "choose at least one agent"));
        }
        if !(input.budget_usd.is_finite()
            && input.budget_usd > 0.0
            && input.budget_usd <= 100_000.0)
        {
            fields.push(FieldError::new("budgetUsd", "must be between 0 and 100000"));
        }
        if input.time_limit_seconds == 0 || input.time_limit_seconds > 14 * 24 * 3600 {
            fields.push(FieldError::new(
                "timeLimitSeconds",
                "must be between 1 s and 14 days",
            ));
        }
        if input.plateau_evals == 0 || input.plateau_evals > 10_000 {
            fields.push(FieldError::new(
                "plateauEvals",
                "must be between 1 and 10000",
            ));
        }
        if input.demo.is_none() && input.repo_url.trim().is_empty() {
            fields.push(FieldError::new("repoUrl", "repoUrl is required"));
        }
        if !fields.is_empty() {
            return Err(ServiceError::InvalidFields {
                message: "the optimization request is invalid".into(),
                fields,
            });
        }
        let (repo, yaml, name) = match input.demo {
            Some(demo) => {
                let (name, yaml) = self.demo_config(demo);
                (
                    format!("acme/{}", demo.as_str()),
                    yaml,
                    Some(input.name.clone().unwrap_or(name)),
                )
            }
            None => {
                let slug =
                    repo_slug(&input.repo_url).ok_or_else(|| ServiceError::InvalidFields {
                        message: format!(
                            "{} is not a GitHub repository (owner/name)",
                            input.repo_url
                        ),
                        fields: vec![FieldError::new("repoUrl", "use owner/name")],
                    })?;
                let missing = self.missing_for_live(&input);
                if !missing.is_empty() {
                    return Err(ServiceError::NotReady(format!(
                        "Not ready: {}",
                        missing.join("; ")
                    )));
                }
                // The repo's own ratchet.yaml when the simulation knows the repo, else the demo config.
                let yaml = self
                    .known_repo_yaml(&slug)
                    .unwrap_or_else(|| DEFAULT_CONFIG_YAML.to_owned());
                (slug, yaml, input.name.clone())
            }
        };
        if let Some(draft) = &input.environment_draft_id {
            self.require_approved_draft(draft)?;
        }
        let cfg = ratchet_core::parse_config(&yaml).map_err(|e| {
            ServiceError::Invalid(format!("invalid ratchet.yaml: {}", join_errors(&e)))
        })?;
        let mut create = new_optimization_input(
            &repo,
            &input.base_ref,
            &cfg,
            &yaml,
            input.budget_usd,
            input.time_limit_seconds as i64,
            name,
        );
        create.status = Some(OptimizationStatus::Draft);
        let opt = self.ledger.create_optimization(create).await?;
        for (i, kind) in input.agents.iter().enumerate() {
            let short = opt.id.as_str().chars().skip(4).take(8).collect::<String>();
            self.ledger
                .add_agent(AddAgentInput::new(
                    opt.id.clone(),
                    *kind,
                    format!("ratchet/{short}/{kind}-{}", i + 1),
                ))
                .await?;
        }
        self.st()
            .plateau
            .insert(opt.id.to_string(), input.plateau_evals);
        self.emit(ApiEvent::Optimization {
            optimization_id: opt.id.to_string(),
            status: OptimizationStatus::Draft,
        });
        Ok(CreatedOptimization {
            id: opt.id.to_string(),
        })
    }

    fn demo_config(&self, demo: DemoId) -> (String, String) {
        match demo {
            DemoId::Search => (
                "Search API latency (demo)".into(),
                DEFAULT_CONFIG_YAML.to_owned(),
            ),
            DemoId::Cli => {
                let r = ratchet_core::render_template("cli-cold-start", &Default::default());
                (
                    "CLI cold start (demo)".into(),
                    r.map(|r| r.files[0].content.clone())
                        .unwrap_or_else(|_| DEFAULT_CONFIG_YAML.to_owned()),
                )
            }
            DemoId::Forecast => {
                let r = ratchet_core::render_template("forecast-backtest", &Default::default());
                (
                    "Demand forecast (demo)".into(),
                    r.map(|r| r.files[0].content.clone())
                        .unwrap_or_else(|_| DEFAULT_CONFIG_YAML.to_owned()),
                )
            }
        }
    }
}

pub(crate) fn join_errors(errors: &[ratchet_core::config::ConfigError]) -> String {
    errors
        .iter()
        .map(|e| e.message.clone())
        .collect::<Vec<_>>()
        .join("; ")
}
