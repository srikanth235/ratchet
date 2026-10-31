//! Run a full Ratchet optimization in simulation mode against a shipped demo. Port of the `simulate()` function of
//! `packages/runtime/src/bin/simulate.ts` (the command line wrapper is `ratchetd`'s / the CLI's job).
//!
//! The scratch directory (`.ratchet-sim-*` in the repo root, holding the git remote, artifacts and PR) is never deleted. The ledger
//! lives there too unless `db_path` puts it somewhere that outlives the scratch dir.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use ratchet_core::config::RatchetConfig;
use ratchet_core::enums::{AgentKind, OptimizationStatus};
use ratchet_core::ids::OptimizationId;
use ratchet_core::ports::{AgentEventType, CancellationToken, EngineEvent};
use ratchet_core::rows::{CandidateWithResults, OptimizationSummary, RatchetHistoryEntry};
use ratchet_core::value::{ValueEstimate, estimate_value, return_on_spend};
use ratchet_ledger::{Ledger, LedgerOptions};
use ratchet_sandbox::MockCloudCompute;

use super::simulation::{SimulationOptions, SimulationProject, create_simulation_runtime};
use crate::engine::{CreateOptimizationInput, Engine};
use crate::errors::{EngineError, EngineResult};
use crate::settings::EngineSettings;

#[derive(Clone)]
pub struct SimulateOptions {
    pub repo_root: PathBuf,
    pub db_path: Option<PathBuf>,
    pub project: SimulationProject,
    /// Keep only the first N attempts of each agent's script.
    pub attempts: Option<usize>,
    /// Load-generator duration per bench run, seconds. Default 3.
    pub duration_sec: u32,
    /// Evaluator repeats. Default 2.
    pub repeats: u32,
    /// Time limit of the optimization. Default 20.
    pub minutes: u64,
    /// Which agents run. Default both (claude-code and codex).
    pub agents: Vec<AgentKind>,
    pub settings: EngineSettings,
    pub tsc: Option<PathBuf>,
    pub bin_dir: Option<PathBuf>,
    pub log: Option<Arc<dyn Fn(&str) + Send + Sync>>,
}

impl SimulateOptions {
    pub fn new(repo_root: impl Into<PathBuf>) -> Self {
        Self {
            repo_root: repo_root.into(),
            db_path: None,
            project: SimulationProject::Search,
            attempts: None,
            duration_sec: 3,
            repeats: 2,
            minutes: 20,
            agents: vec![AgentKind::ClaudeCode, AgentKind::Codex],
            settings: EngineSettings::default(),
            tsc: None,
            bin_dir: None,
            log: None,
        }
    }
}

pub struct SimulateResult {
    pub id: String,
    /// After teardown: the final status (`completed`) and the spend including the last metering pass.
    pub summary: OptimizationSummary,
    pub history: Vec<RatchetHistoryEntry>,
    pub candidates: Vec<CandidateWithResults>,
    pub pr: Option<String>,
    pub work_root: PathBuf,
    pub db_path: PathBuf,
    pub ledger: Ledger,
    pub compute: Arc<MockCloudCompute>,
    pub engine: Engine,
    pub config: RatchetConfig,
    pub value: Option<ValueEstimate>,
    pub roi: Option<f64>,
}

pub async fn simulate(o: SimulateOptions) -> EngineResult<SimulateResult> {
    let log = o
        .log
        .clone()
        .unwrap_or_else(|| Arc::new(|s| println!("{s}")));
    let work_root = tempfile::Builder::new()
        .prefix(".ratchet-sim-")
        .tempdir_in(&o.repo_root)
        .map_err(|e| EngineError::internal(e.to_string()))?
        .keep();
    let db_path = o.db_path.clone().map_or_else(
        || work_root.join("ratchet.db"),
        |p| std::path::absolute(&p).unwrap_or(p),
    );
    if let Some(dir) = db_path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| EngineError::internal(e.to_string()))?;
    }
    let ledger = Ledger::open(&db_path, LedgerOptions::default()).await?;
    let project = o.project;
    let trim = |v: ratchet_agents::DemoVariant| -> EngineResult<ratchet_agents::AgentScript> {
        let mut s = project
            .scripts(v)
            .map_err(|e| EngineError::invalid_input(e.to_string()))?;
        if let Some(n) = o.attempts.filter(|n| *n > 0) {
            s.attempts.truncate(n);
        }
        Ok(s)
    };
    let mut so = SimulationOptions::new(ledger.clone(), &work_root, &o.repo_root);
    so.project = project;
    so.bench_duration_sec = Some(o.duration_sec);
    so.repeats = Some(o.repeats);
    so.scripts.insert(
        AgentKind::ClaudeCode,
        trim(ratchet_agents::DemoVariant::Cc)?,
    );
    so.scripts
        .insert(AgentKind::Codex, trim(ratchet_agents::DemoVariant::Cx)?);
    so.settings = o.settings.clone();
    so.tsc = o.tsc.clone();
    so.bin_dir = o.bin_dir.clone();
    let rt = create_simulation_runtime(so).await?;

    let mut rx = rt.engine.subscribe();
    let logger = {
        let log = log.clone();
        tokio::spawn(async move {
            while let Ok(e) = rx.recv().await {
                match &e {
                    EngineEvent::Spend { .. } => {} // one per metering tick: too noisy for the console
                    EngineEvent::AgentEvent {
                        agent_id, event, ..
                    } => {
                        if matches!(
                            event.event_type,
                            AgentEventType::Commit
                                | AgentEventType::Verdict
                                | AgentEventType::Error
                        ) {
                            log(&format!(
                                "  [{agent_id}] {}: {}",
                                format!("{:?}", event.event_type).to_lowercase(),
                                event.text.as_deref().unwrap_or("")
                            ));
                        }
                    }
                    other => log(&format!(
                        "{}",
                        serde_json::to_string(other)
                            .unwrap_or_default()
                            .chars()
                            .take(200)
                            .collect::<String>()
                    )),
                }
            }
        })
    };
    let id = rt
        .engine
        .create_optimization(CreateOptimizationInput {
            name: Some(format!(
                "{} (simulation)",
                rt.config.objective.name.clone().unwrap_or_else(|| rt
                    .config
                    .objective
                    .metric
                    .clone())
            )),
            repo_url: rt.repo_url.clone(),
            base_ref: rt.base_ref.clone(),
            agents: o.agents.clone(),
            budget_usd: 50.0,
            time_limit_seconds: o.minutes * 60,
            plateau_evals: 50,
            ..Default::default()
        })
        .await?;
    let oid = OptimizationId::new(&id);
    let cancel = CancellationToken::new();
    let running = {
        let (e, c) = (rt.engine.clone(), cancel.clone());
        tokio::spawn(async move { e.run(Some(c)).await })
    };
    rt.engine.start(&id).await?;
    let deadline = Instant::now() + Duration::from_secs(o.minutes * 60 + 120);
    loop {
        let opt = ledger.get_optimization(&oid).await?;
        if opt.is_some_and(|o| {
            matches!(
                o.status,
                OptimizationStatus::Completed
                    | OptimizationStatus::Stopped
                    | OptimizationStatus::Failed
            )
        }) {
            break;
        }
        if Instant::now() > deadline {
            log("simulation timed out; stopping");
            rt.engine.stop(&id, Some("simulation timeout")).await?;
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }
    let summarize = || async {
        ledger
            .list_optimizations(None)
            .await?
            .into_iter()
            .find(|r| r.id == oid)
            .ok_or_else(|| EngineError::not_found(format!("optimization {id}")))
    };
    let wins = summarize().await?.verified_wins;
    let history = ledger.ratchet_history(&oid).await?;
    let candidates = ledger.candidates_with_results(&oid).await?;
    let pr = if wins > 0 {
        Some(rt.engine.open_pull_request(&id).await?.url)
    } else {
        None
    };
    if let Err(e) = rt.engine.teardown(&id).await {
        log(&format!("teardown: {}", e.message));
    }
    cancel.cancel();
    let _ = running.await;
    logger.abort();
    rt.dispose().await;
    let summary = summarize().await?;
    let value = match (summary.baseline_value, summary.best_value) {
        (Some(b), Some(best)) => estimate_value(&rt.config.objective, b, best),
        _ => None,
    };
    let roi = value
        .as_ref()
        .and_then(|v| return_on_spend(v.usd_per_year, summary.spend_usd));
    Ok(SimulateResult {
        id,
        summary,
        history,
        candidates,
        pr,
        work_root,
        db_path,
        ledger,
        compute: rt.compute.clone(),
        engine: rt.engine.clone(),
        config: rt.config.clone(),
        value,
        roi,
    })
}
