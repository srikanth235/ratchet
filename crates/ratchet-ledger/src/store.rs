//! The ledger: the single source of truth for an optimization. SQLite through sqlx.
//!
//! Port of `packages/core/src/ledger/{db,store}.ts`. `ratchet_moves` and `spend_events` are append-only
//! (enforced by triggers in the migration). Every method is async; write sequences that must be atomic run in
//! a `BEGIN IMMEDIATE` transaction.

use std::collections::{BTreeSet, HashMap};
use std::future::Future;
use std::path::Path;
use std::pin::Pin;
use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;

use ratchet_core::config::GuardrailResult;
use ratchet_core::enums::{
    AgentKind, AgentStatus, BenchPhase, CandidateVerdict, Direction, EvaluatorKind,
    OptimizationStatus, SandboxStatus, SpendCategory, ValueBasis, ValuePeriod,
};
use ratchet_core::ids::{AgentId, CandidateId, OptimizationId, SandboxId};
use ratchet_core::ledger::{LedgerError, Patch, RecordSandboxInput, SandboxStatusPatch};
use ratchet_core::ports::{Clock, CostReconcileLine};
use ratchet_core::rows::{
    ActivityItem, Agent, AgentEventRecord, AgentSpend, Baseline, BenchResult, BenchStats,
    Candidate, CandidateWithResults, CategoryTotals, Optimization, OptimizationSummary,
    RatchetHistoryEntry, RatchetMove, Sandbox, SpendBreakdown, SpendEvent, gain_percent,
    is_improvement,
};
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions};
use sqlx::{SqliteConnection, SqlitePool};

use crate::rowmap::{self, Result, col, col_enum, db, json_text};

/// A future boxed for use in [`Ledger::transaction`].
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Options of [`Ledger::open`].
#[derive(Clone, Default)]
pub struct LedgerOptions {
    /// Clock (epoch ms). Injectable for tests. Default: the system clock.
    pub clock: Option<Arc<dyn Clock>>,
    /// Connections in the pool for a file database (default 4; an in-memory database always uses one).
    pub max_connections: Option<u32>,
}

// ---- inputs ---------------------------------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct CreateOptimizationInput {
    pub id: Option<OptimizationId>,
    pub name: Option<String>,
    pub repo: String,
    pub base_ref: String,
    pub metric: String,
    pub direction: Direction,
    pub min_gain: f64,
    pub budget_usd: f64,
    pub time_limit_s: i64,
    pub config_yaml: String,
    pub status: Option<OptimizationStatus>,
    pub created_at: Option<i64>,
    pub pack: Option<String>,
    pub objective_name: Option<String>,
    pub evaluator_kind: Option<EvaluatorKind>,
    /// `objective.value` from ratchet.yaml; `None` when the config has no value model.
    pub value: Option<ValueInput>,
    pub sandbox_id: Option<SandboxId>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ValueInput {
    pub usd_per_unit: f64,
    pub period: ValuePeriod,
    pub basis: ValueBasis,
    pub unit: Option<String>,
}

impl CreateOptimizationInput {
    /// The required fields; set the optional ones on the returned value.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repo: impl Into<String>,
        base_ref: impl Into<String>,
        metric: impl Into<String>,
        direction: Direction,
        min_gain: f64,
        budget_usd: f64,
        time_limit_s: i64,
        config_yaml: impl Into<String>,
    ) -> Self {
        Self {
            id: None,
            name: None,
            repo: repo.into(),
            base_ref: base_ref.into(),
            metric: metric.into(),
            direction,
            min_gain,
            budget_usd,
            time_limit_s,
            config_yaml: config_yaml.into(),
            status: None,
            created_at: None,
            pack: None,
            objective_name: None,
            evaluator_kind: None,
            value: None,
            sandbox_id: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordBaselineInput {
    pub optimization_id: OptimizationId,
    pub samples: Vec<f64>,
    pub noise_floor: f64,
    pub stats: BenchStats,
    /// Headline baseline value; defaults to `stats.value`.
    pub value: Option<f64>,
    pub recorded_at: Option<i64>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AddAgentInput {
    pub id: Option<AgentId>,
    pub optimization_id: OptimizationId,
    pub kind: AgentKind,
    pub branch: String,
    pub runner_instance_id: Option<String>,
    pub status: Option<AgentStatus>,
    pub created_at: Option<i64>,
}

impl AddAgentInput {
    pub fn new(
        optimization_id: impl Into<OptimizationId>,
        kind: AgentKind,
        branch: impl Into<String>,
    ) -> Self {
        Self {
            id: None,
            optimization_id: optimization_id.into(),
            kind,
            branch: branch.into(),
            runner_instance_id: None,
            status: None,
            created_at: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct SubmitCandidateInput {
    pub id: Option<CandidateId>,
    pub optimization_id: OptimizationId,
    pub agent_id: AgentId,
    pub sha: String,
    pub message: Option<String>,
    pub submitted_at: Option<i64>,
}

impl SubmitCandidateInput {
    pub fn new(
        optimization_id: impl Into<OptimizationId>,
        agent_id: impl Into<AgentId>,
        sha: impl Into<String>,
    ) -> Self {
        Self {
            id: None,
            optimization_id: optimization_id.into(),
            agent_id: agent_id.into(),
            sha: sha.into(),
            message: None,
            submitted_at: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordBenchResultInput {
    pub candidate_id: CandidateId,
    pub phase: BenchPhase,
    pub samples: Vec<f64>,
    pub stats: BenchStats,
    pub guardrails: Vec<GuardrailResult>,
    pub started_at: i64,
    pub finished_at: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RecordSpendInput {
    pub optimization_id: OptimizationId,
    pub agent_id: Option<AgentId>,
    pub category: SpendCategory,
    pub usd: f64,
    /// True for live estimates, false once reconciled against billing. Default true.
    pub estimated: Option<bool>,
    pub source: String,
    pub at: Option<i64>,
}

impl RecordSpendInput {
    pub fn new(
        optimization_id: impl Into<OptimizationId>,
        category: SpendCategory,
        usd: f64,
        source: impl Into<String>,
    ) -> Self {
        Self {
            optimization_id: optimization_id.into(),
            agent_id: None,
            category,
            usd,
            estimated: None,
            source: source.into(),
            at: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AppendAgentEventInput {
    pub agent_id: AgentId,
    pub event_type: String,
    pub payload: Option<serde_json::Value>,
    pub at: Option<i64>,
}

// ---- helpers --------------------------------------------------------------------------------------------------

fn assert_finite(name: &str, v: f64) -> Result<()> {
    if v.is_finite() {
        Ok(())
    } else {
        Err(LedgerError::invalid(format!(
            "{name} must be a finite number"
        )))
    }
}

/// A string that is safe to splice into SQL: only ever built from enum wire values.
fn status_list(statuses: &[OptimizationStatus]) -> sqlx::AssertSqlSafe<String> {
    let quoted: Vec<String> = statuses
        .iter()
        .map(|s| format!("'{}'", s.as_str()))
        .collect();
    sqlx::AssertSqlSafe(format!(
        "SELECT * FROM optimizations WHERE status IN ({}) ORDER BY created_at DESC, id DESC",
        quoted.join(", ")
    ))
}

// ---- the ledger -----------------------------------------------------------------------------------------------

/// The ledger handle: cheap to clone (it shares the connection pool).
#[derive(Clone)]
pub struct Ledger {
    pool: SqlitePool,
    clock: Arc<dyn Clock>,
}

impl Ledger {
    /// Open (creating if needed) a ledger database file and apply the migration.
    pub async fn open(path: impl AsRef<Path>, options: LedgerOptions) -> Result<Ledger> {
        let opts = SqliteConnectOptions::new()
            .filename(path.as_ref())
            .create_if_missing(true)
            .foreign_keys(true)
            .journal_mode(SqliteJournalMode::Wal)
            .busy_timeout(Duration::from_millis(5000));
        let pool = SqlitePoolOptions::new()
            .max_connections(options.max_connections.unwrap_or(4))
            .connect_with(opts)
            .await
            .map_err(db)?;
        Self::finish(pool, options).await
    }

    /// An in-memory ledger (one connection; gone when the last handle drops).
    pub async fn open_memory(options: LedgerOptions) -> Result<Ledger> {
        let opts = SqliteConnectOptions::from_str("sqlite::memory:")
            .map_err(db)?
            .foreign_keys(true)
            .busy_timeout(Duration::from_millis(5000));
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(opts)
            .await
            .map_err(db)?;
        Self::finish(pool, options).await
    }

    async fn finish(pool: SqlitePool, options: LedgerOptions) -> Result<Ledger> {
        if let Err(e) = sqlx::migrate!("./migrations").run(&pool).await {
            pool.close().await;
            return Err(LedgerError::database(format!(
                "ledger migration failed: {e}"
            )));
        }
        let clock = options
            .clock
            .unwrap_or_else(|| Arc::new(ratchet_core::ports::mock::SystemClock));
        Ok(Ledger { pool, clock })
    }

    /// The connection pool, for raw SQL in tests and diagnostics.
    pub fn pool(&self) -> &SqlitePool {
        &self.pool
    }

    pub async fn close(&self) {
        self.pool.close().await;
    }

    fn now(&self) -> i64 {
        self.clock.now()
    }

    async fn conn(&self) -> Result<sqlx::pool::PoolConnection<sqlx::Sqlite>> {
        self.pool.acquire().await.map_err(db)
    }

    /// Run `f` in a single write transaction (`BEGIN IMMEDIATE`): committed when it returns `Ok`, rolled back
    /// when it returns `Err` (or panics).
    pub async fn transaction<T, F>(&self, f: F) -> Result<T>
    where
        F: for<'c> FnOnce(&'c mut SqliteConnection) -> BoxFuture<'c, Result<T>>,
    {
        let mut tx = self.pool.begin_with("BEGIN IMMEDIATE").await.map_err(db)?;
        match f(&mut tx).await {
            Ok(v) => {
                tx.commit().await.map_err(db)?;
                Ok(v)
            }
            Err(e) => {
                let _ = tx.rollback().await;
                Err(e)
            }
        }
    }

    /// [`transaction`](Self::transaction) with the typed writes of [`Tx`]:
    /// `ledger.atomically(|tx| Box::pin(async move { ... }))`.
    pub async fn atomically<T, F>(&self, f: F) -> Result<T>
    where
        F: for<'c> FnOnce(Tx<'c>) -> BoxFuture<'c, Result<T>> + Send + 'static,
        T: Send,
    {
        let now = self.now();
        self.transaction(move |conn| Box::pin(async move { f(Tx { conn, now }).await }))
            .await
    }

    // ---- optimizations ----------------------------------------------------------------------------------------

    pub async fn create_optimization(
        &self,
        input: CreateOptimizationInput,
    ) -> Result<Optimization> {
        create_optimization(&mut *self.conn().await?, input, self.now()).await
    }

    pub async fn get_optimization(&self, id: &OptimizationId) -> Result<Option<Optimization>> {
        get_optimization(&mut *self.conn().await?, id.as_str()).await
    }

    /// Move an optimization through its lifecycle, stamping started/ended times. `at` defaults to now.
    pub async fn set_optimization_status(
        &self,
        id: &OptimizationId,
        status: OptimizationStatus,
        at: Option<i64>,
    ) -> Result<Optimization> {
        let at = at.unwrap_or_else(|| self.now());
        let mut conn = self.conn().await?;
        let cur = require_optimization(&mut conn, id.as_str()).await?;
        let started = if status == OptimizationStatus::Running && cur.started_at.is_none() {
            Some(at)
        } else {
            cur.started_at
        };
        let ended = if status.is_terminal() {
            Some(at)
        } else {
            cur.ended_at
        };
        let row = sqlx::query("UPDATE optimizations SET status = ?1, started_at = ?2, ended_at = ?3 WHERE id = ?4 RETURNING *")
            .bind(status.as_str())
            .bind(started)
            .bind(ended)
            .bind(id.as_str())
            .fetch_one(&mut *conn)
            .await
            .map_err(db)?;
        rowmap::optimization(&row)
    }

    /// Point an optimization at the sandbox it runs in (the engine creates the sandbox lazily, at the first run start).
    pub async fn set_optimization_sandbox(
        &self,
        id: &OptimizationId,
        sandbox_id: Option<&SandboxId>,
    ) -> Result<Optimization> {
        let mut conn = self.conn().await?;
        require_optimization(&mut conn, id.as_str()).await?;
        let row = sqlx::query("UPDATE optimizations SET sandbox_id = ?1 WHERE id = ?2 RETURNING *")
            .bind(sandbox_id.map(SandboxId::as_str))
            .bind(id.as_str())
            .fetch_one(&mut *conn)
            .await
            .map_err(db)?;
        rowmap::optimization(&row)
    }

    /// Optimizations that ran in a sandbox; teardown must not destroy one that is still live.
    pub async fn optimizations_in_sandbox(&self, sandbox_id: &str) -> Result<Vec<Optimization>> {
        let rows = sqlx::query("SELECT * FROM optimizations WHERE sandbox_id = ?1 ORDER BY rowid")
            .bind(sandbox_id)
            .fetch_all(&mut *self.conn().await?)
            .await
            .map_err(db)?;
        rows.iter().map(rowmap::optimization).collect()
    }

    /// Exactly what the Optimizations table shows, newest first. `status`: only these statuses (`None`: all).
    pub async fn list_optimizations(
        &self,
        status: Option<&[OptimizationStatus]>,
    ) -> Result<Vec<OptimizationSummary>> {
        let mut conn = self.conn().await?;
        let rows = match status {
            None => {
                sqlx::query("SELECT * FROM optimizations ORDER BY created_at DESC, id DESC")
                    .fetch_all(&mut *conn)
                    .await
            }
            Some([]) => return Ok(Vec::new()),
            Some(s) => sqlx::query(status_list(s)).fetch_all(&mut *conn).await,
        }
        .map_err(db)?;
        if rows.is_empty() {
            return Ok(Vec::new());
        }
        let opts = rows
            .iter()
            .map(rowmap::optimization)
            .collect::<Result<Vec<_>>>()?;

        let mut baseline_by_opt: HashMap<String, f64> = HashMap::new();
        for r in sqlx::query("SELECT optimization_id, value FROM baselines")
            .fetch_all(&mut *conn)
            .await
            .map_err(db)?
        {
            baseline_by_opt.insert(col(&r, "optimization_id")?, col(&r, "value")?);
        }

        // last move per optimization = current best
        let mut last_move: HashMap<String, f64> = HashMap::new();
        let mut move_count: HashMap<String, i64> = HashMap::new();
        for r in sqlx::query("SELECT optimization_id, to_value FROM ratchet_moves ORDER BY id")
            .fetch_all(&mut *conn)
            .await
            .map_err(db)?
        {
            let o: String = col(&r, "optimization_id")?;
            last_move.insert(o.clone(), col(&r, "to_value")?);
            *move_count.entry(o).or_insert(0) += 1;
        }

        let mut spend: HashMap<String, f64> = HashMap::new();
        for r in sqlx::query("SELECT optimization_id, coalesce(sum(usd), 0.0) AS total FROM spend_events GROUP BY optimization_id")
            .fetch_all(&mut *conn)
            .await
            .map_err(db)?
        {
            spend.insert(col(&r, "optimization_id")?, col(&r, "total")?);
        }

        let mut kinds: HashMap<String, BTreeSet<AgentKind>> = HashMap::new();
        let mut agent_count: HashMap<String, i64> = HashMap::new();
        for r in sqlx::query("SELECT optimization_id, kind FROM agents")
            .fetch_all(&mut *conn)
            .await
            .map_err(db)?
        {
            let o: String = col(&r, "optimization_id")?;
            kinds
                .entry(o.clone())
                .or_default()
                .insert(col_enum(&r, "kind")?);
            *agent_count.entry(o).or_insert(0) += 1;
        }

        let mut candidate_count: HashMap<String, i64> = HashMap::new();
        for r in sqlx::query(
            "SELECT optimization_id, count(*) AS n FROM candidates GROUP BY optimization_id",
        )
        .fetch_all(&mut *conn)
        .await
        .map_err(db)?
        {
            candidate_count.insert(col(&r, "optimization_id")?, col(&r, "n")?);
        }

        Ok(opts
            .into_iter()
            .map(|o| {
                let id = o.id.as_str();
                let baseline_value = baseline_by_opt.get(id).copied();
                let best_value = last_move.get(id).copied().or(baseline_value);
                let gain_pct = match (baseline_value, best_value) {
                    (Some(b), Some(best)) => gain_percent(o.direction, b, best),
                    _ => None,
                };
                // `AgentKind` derives Ord in declaration order; the TypeScript sorted the wire strings
                let mut agent_kinds: Vec<AgentKind> = kinds
                    .get(id)
                    .map(|s| s.iter().copied().collect())
                    .unwrap_or_default();
                agent_kinds.sort_by_key(|k| k.as_str());
                OptimizationSummary {
                    baseline_value,
                    best_value,
                    gain_pct,
                    spend_usd: spend.get(id).copied().unwrap_or(0.0),
                    agent_kinds,
                    agent_count: agent_count.get(id).copied().unwrap_or(0),
                    candidate_count: candidate_count.get(id).copied().unwrap_or(0),
                    verified_wins: move_count.get(id).copied().unwrap_or(0),
                    id: o.id.clone(),
                    name: o.name.clone(),
                    repo: o.repo.clone(),
                    metric: o.metric.clone(),
                    direction: o.direction,
                    status: o.status,
                    budget_usd: o.budget_usd,
                    created_at: o.created_at,
                    started_at: o.started_at,
                    ended_at: o.ended_at,
                }
            })
            .collect())
    }

    // ---- sandboxes --------------------------------------------------------------------------------------------

    /// Register a sandbox (idempotent on id: an existing row is returned unchanged).
    pub async fn record_sandbox(&self, input: RecordSandboxInput) -> Result<Sandbox> {
        let mut conn = self.conn().await?;
        if let Some(existing) = get_sandbox(&mut conn, input.id.as_str()).await? {
            return Ok(existing);
        }
        let row = sqlx::query(
            "INSERT INTO sandboxes (id, provider, region, account_id, status, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6) RETURNING *",
        )
        .bind(input.id.as_str())
        .bind(input.provider.as_str())
        .bind(&input.region)
        .bind(&input.account_id)
        .bind(input.status.unwrap_or(SandboxStatus::Planned).as_str())
        .bind(input.created_at.unwrap_or_else(|| self.now()))
        .fetch_one(&mut *conn)
        .await
        .map_err(db)?;
        rowmap::sandbox(&row)
    }

    pub async fn get_sandbox(&self, id: &str) -> Result<Option<Sandbox>> {
        get_sandbox(&mut *self.conn().await?, id).await
    }

    /// Newest first.
    pub async fn list_sandboxes(&self) -> Result<Vec<Sandbox>> {
        let rows = sqlx::query("SELECT * FROM sandboxes ORDER BY created_at DESC, rowid DESC")
            .fetch_all(&mut *self.conn().await?)
            .await
            .map_err(db)?;
        rows.iter().map(rowmap::sandbox).collect()
    }

    /// Move a sandbox through apply/destroy, stamping times. `outputs` is stored on apply (`Patch::Clear` clears
    /// them on destroy); `account_id` is filled in once the account is known.
    pub async fn set_sandbox_status(
        &self,
        id: &str,
        status: SandboxStatus,
        patch: SandboxStatusPatch,
    ) -> Result<Sandbox> {
        let mut conn = self.conn().await?;
        let Some(cur) = get_sandbox(&mut conn, id).await? else {
            return Err(LedgerError::not_found(format!("sandbox {id} not found")));
        };
        let at = patch.at.unwrap_or_else(|| self.now());
        let applied_at = if status == SandboxStatus::Applied {
            Some(at)
        } else {
            cur.applied_at
        };
        let destroyed_at = if status == SandboxStatus::Destroyed {
            Some(at)
        } else {
            cur.destroyed_at
        };
        let outputs = match patch.outputs {
            Patch::Keep => cur.outputs,
            Patch::Set(v) => Some(v),
            Patch::Clear => None,
        };
        let account_id = match patch.account_id {
            Patch::Keep => cur.account_id,
            Patch::Set(v) => Some(v),
            Patch::Clear => None,
        };
        let outputs_text = outputs.as_ref().map(json_text).transpose()?;
        let row = sqlx::query(
            "UPDATE sandboxes SET status = ?1, applied_at = ?2, destroyed_at = ?3, outputs = ?4, account_id = ?5 WHERE id = ?6 RETURNING *",
        )
        .bind(status.as_str())
        .bind(applied_at)
        .bind(destroyed_at)
        .bind(outputs_text)
        .bind(account_id)
        .bind(id)
        .fetch_one(&mut *conn)
        .await
        .map_err(db)?;
        rowmap::sandbox(&row)
    }

    // ---- baseline ---------------------------------------------------------------------------------------------

    /// Record (or re-record, before any ratchet move) the baseline.
    pub async fn record_baseline(&self, input: RecordBaselineInput) -> Result<Baseline> {
        let id = input.optimization_id.clone();
        require_optimization(&mut *self.conn().await?, id.as_str()).await?;
        assert_finite("noiseFloor", input.noise_floor)?;
        let value = input.value.unwrap_or(input.stats.value);
        assert_finite("baseline value", value)?;
        let samples = json_text(&input.samples)?;
        let stats = json_text(&input.stats)?;
        let recorded_at = input.recorded_at.unwrap_or_else(|| self.now());
        self.transaction(move |tx| {
            Box::pin(async move {
                let moved = sqlx::query("SELECT id FROM ratchet_moves WHERE optimization_id = ?1 LIMIT 1")
                    .bind(id.as_str())
                    .fetch_optional(&mut *tx)
                    .await
                    .map_err(db)?;
                if moved.is_some() {
                    return Err(LedgerError::invalid("cannot change the baseline after the ratchet has moved"));
                }
                let row = sqlx::query(
                    "INSERT INTO baselines (optimization_id, samples, value, noise_floor, stats, recorded_at) \
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6) \
                     ON CONFLICT(optimization_id) DO UPDATE SET samples = excluded.samples, value = excluded.value, \
                     noise_floor = excluded.noise_floor, stats = excluded.stats, recorded_at = excluded.recorded_at \
                     RETURNING *",
                )
                .bind(id.as_str())
                .bind(samples)
                .bind(value)
                .bind(input.noise_floor)
                .bind(stats)
                .bind(recorded_at)
                .fetch_one(&mut *tx)
                .await
                .map_err(db)?;
                rowmap::baseline(&row)
            })
        })
        .await
    }

    pub async fn get_baseline(&self, optimization_id: &OptimizationId) -> Result<Option<Baseline>> {
        get_baseline(&mut *self.conn().await?, optimization_id.as_str()).await
    }

    // ---- agents -----------------------------------------------------------------------------------------------

    pub async fn add_agent(&self, input: AddAgentInput) -> Result<Agent> {
        let mut conn = self.conn().await?;
        require_optimization(&mut conn, input.optimization_id.as_str()).await?;
        let id = input.id.unwrap_or_else(AgentId::generate);
        let row = sqlx::query(
            "INSERT INTO agents (id, optimization_id, kind, branch, runner_instance_id, status, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) RETURNING *",
        )
        .bind(id.as_str())
        .bind(input.optimization_id.as_str())
        .bind(input.kind.as_str())
        .bind(&input.branch)
        .bind(&input.runner_instance_id)
        .bind(input.status.unwrap_or(AgentStatus::Pending).as_str())
        .bind(input.created_at.unwrap_or_else(|| self.now()))
        .fetch_one(&mut *conn)
        .await
        .map_err(db)?;
        rowmap::agent(&row)
    }

    pub async fn get_agent(&self, id: &AgentId) -> Result<Option<Agent>> {
        get_agent(&mut *self.conn().await?, id.as_str()).await
    }

    /// Agents of one optimization in the order they were added.
    pub async fn list_agents(&self, optimization_id: &OptimizationId) -> Result<Vec<Agent>> {
        list_agents(&mut *self.conn().await?, optimization_id.as_str()).await
    }

    /// `runner_instance_id`: `Keep` leaves it alone, `Set`/`Clear` change it.
    pub async fn set_agent_status(
        &self,
        id: &AgentId,
        status: AgentStatus,
        runner_instance_id: Patch<String>,
    ) -> Result<Agent> {
        let mut conn = self.conn().await?;
        let cur = require_agent(&mut conn, id.as_str()).await?;
        let runner = match runner_instance_id {
            Patch::Keep => cur.runner_instance_id,
            Patch::Set(v) => Some(v),
            Patch::Clear => None,
        };
        let row = sqlx::query(
            "UPDATE agents SET status = ?1, runner_instance_id = ?2 WHERE id = ?3 RETURNING *",
        )
        .bind(status.as_str())
        .bind(runner)
        .bind(id.as_str())
        .fetch_one(&mut *conn)
        .await
        .map_err(db)?;
        rowmap::agent(&row)
    }

    // ---- candidates -------------------------------------------------------------------------------------------

    pub async fn submit_candidate(&self, input: SubmitCandidateInput) -> Result<Candidate> {
        let mut conn = self.conn().await?;
        let agent = require_agent(&mut conn, input.agent_id.as_str()).await?;
        if agent.optimization_id != input.optimization_id {
            return Err(LedgerError::invalid(format!(
                "agent {} does not belong to optimization {}",
                input.agent_id, input.optimization_id
            )));
        }
        let id = input.id.unwrap_or_else(CandidateId::generate);
        let row = sqlx::query(
            "INSERT INTO candidates (id, optimization_id, agent_id, sha, message, submitted_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6) RETURNING *",
        )
        .bind(id.as_str())
        .bind(input.optimization_id.as_str())
        .bind(input.agent_id.as_str())
        .bind(&input.sha)
        .bind(input.message.as_deref().unwrap_or(""))
        .bind(input.submitted_at.unwrap_or_else(|| self.now()))
        .fetch_one(&mut *conn)
        .await
        .map_err(db)?;
        rowmap::candidate(&row)
    }

    pub async fn get_candidate(&self, id: &CandidateId) -> Result<Option<Candidate>> {
        get_candidate(&mut *self.conn().await?, id.as_str()).await
    }

    pub async fn record_bench_result(&self, input: RecordBenchResultInput) -> Result<BenchResult> {
        let mut conn = self.conn().await?;
        require_candidate(&mut conn, input.candidate_id.as_str()).await?;
        assert_finite("stats.value", input.stats.value)?;
        let row = sqlx::query(
            "INSERT INTO bench_results (candidate_id, phase, samples, stats, guardrails, started_at, finished_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) RETURNING *",
        )
        .bind(input.candidate_id.as_str())
        .bind(input.phase.as_str())
        .bind(json_text(&input.samples)?)
        .bind(json_text(&input.stats)?)
        .bind(json_text(&input.guardrails)?)
        .bind(input.started_at)
        .bind(input.finished_at)
        .fetch_one(&mut *conn)
        .await
        .map_err(db)?;
        rowmap::bench_result(&row)
    }

    /// Set a candidate's verdict and reasons; `verdict_at` is `at` (default now), or null for `pending`.
    pub async fn set_verdict(
        &self,
        candidate_id: &CandidateId,
        verdict: CandidateVerdict,
        reasons: &[String],
        at: Option<i64>,
    ) -> Result<Candidate> {
        let mut conn = self.conn().await?;
        require_candidate(&mut conn, candidate_id.as_str()).await?;
        let at = at.unwrap_or_else(|| self.now());
        let row = sqlx::query("UPDATE candidates SET verdict = ?1, reasons = ?2, verdict_at = ?3 WHERE id = ?4 RETURNING *")
            .bind(verdict.as_str())
            .bind(json_text(reasons)?)
            .bind((verdict != CandidateVerdict::Pending).then_some(at))
            .bind(candidate_id.as_str())
            .fetch_one(&mut *conn)
            .await
            .map_err(db)?;
        rowmap::candidate(&row)
    }

    /// All candidates of an optimization (oldest first) with their bench/confirm results.
    pub async fn candidates_with_results(
        &self,
        optimization_id: &OptimizationId,
    ) -> Result<Vec<CandidateWithResults>> {
        let mut conn = self.conn().await?;
        let rows = sqlx::query(
            "SELECT c.*, a.kind AS agent_kind, a.branch AS agent_branch FROM candidates c \
             INNER JOIN agents a ON a.id = c.agent_id WHERE c.optimization_id = ?1 \
             ORDER BY c.submitted_at ASC, c.id ASC",
        )
        .bind(optimization_id.as_str())
        .fetch_all(&mut *conn)
        .await
        .map_err(db)?;
        if rows.is_empty() {
            return Ok(Vec::new());
        }

        let mut results_by_cand: HashMap<String, Vec<BenchResult>> = HashMap::new();
        for r in sqlx::query(
            "SELECT b.* FROM bench_results b INNER JOIN candidates c ON c.id = b.candidate_id \
             WHERE c.optimization_id = ?1 ORDER BY b.id ASC",
        )
        .bind(optimization_id.as_str())
        .fetch_all(&mut *conn)
        .await
        .map_err(db)?
        {
            let res = rowmap::bench_result(&r)?;
            results_by_cand
                .entry(res.candidate_id.to_string())
                .or_default()
                .push(res);
        }
        let moved: std::collections::HashSet<String> =
            sqlx::query("SELECT candidate_id FROM ratchet_moves WHERE optimization_id = ?1")
                .bind(optimization_id.as_str())
                .fetch_all(&mut *conn)
                .await
                .map_err(db)?
                .iter()
                .map(|r| col::<String>(r, "candidate_id"))
                .collect::<Result<_>>()?;

        rows.iter()
            .map(|r| {
                let candidate = rowmap::candidate(r)?;
                let results = results_by_cand
                    .remove(candidate.id.as_str())
                    .unwrap_or_default();
                let latest = |phase: BenchPhase| {
                    results
                        .iter()
                        .rev()
                        .find(|x| x.phase == phase)
                        .map(|x| x.stats.value)
                };
                Ok(CandidateWithResults {
                    agent_kind: col_enum(r, "agent_kind")?,
                    agent_branch: col(r, "agent_branch")?,
                    bench_value: latest(BenchPhase::Bench),
                    confirm_value: latest(BenchPhase::Confirm),
                    moved_ratchet: moved.contains(candidate.id.as_str()),
                    results,
                    candidate,
                })
            })
            .collect()
    }

    // ---- ratchet ----------------------------------------------------------------------------------------------

    /// Current ratchet value: last move's `to_value`, else the baseline, else `None`.
    pub async fn current_best(&self, optimization_id: &OptimizationId) -> Result<Option<f64>> {
        current_best(&mut *self.conn().await?, optimization_id.as_str()).await
    }

    /// Advance the ratchet to `to_value` (attributed to `candidate_id`). The ratchet only turns one way: rejects
    /// a value that is not strictly better than the current best in the optimization's direction.
    pub async fn move_ratchet(
        &self,
        optimization_id: &OptimizationId,
        candidate_id: &CandidateId,
        to_value: f64,
        at: Option<i64>,
    ) -> Result<RatchetMove> {
        assert_finite("toValue", to_value)?;
        let at = at.unwrap_or_else(|| self.now());
        let (opt_id, cand_id) = (optimization_id.clone(), candidate_id.clone());
        self.transaction(move |tx| {
            Box::pin(async move { move_ratchet(tx, &opt_id, &cand_id, to_value, at).await })
        })
        .await
    }

    pub async fn ratchet_history(
        &self,
        optimization_id: &OptimizationId,
    ) -> Result<Vec<RatchetHistoryEntry>> {
        let mut conn = self.conn().await?;
        let opt = require_optimization(&mut conn, optimization_id.as_str()).await?;
        let rows = sqlx::query(
            "SELECT m.id, m.optimization_id, m.candidate_id, c.sha, c.agent_id, a.kind AS agent_kind, m.from_value, m.to_value, m.at \
             FROM ratchet_moves m INNER JOIN candidates c ON c.id = m.candidate_id INNER JOIN agents a ON a.id = c.agent_id \
             WHERE m.optimization_id = ?1 ORDER BY m.id ASC",
        )
        .bind(optimization_id.as_str())
        .fetch_all(&mut *conn)
        .await
        .map_err(db)?;
        rows.iter()
            .map(|r| {
                let (from_value, to_value): (f64, f64) =
                    (col(r, "from_value")?, col(r, "to_value")?);
                Ok(RatchetHistoryEntry {
                    id: col(r, "id")?,
                    optimization_id: col::<String>(r, "optimization_id")?.into(),
                    candidate_id: col::<String>(r, "candidate_id")?.into(),
                    sha: col(r, "sha")?,
                    agent_id: col::<String>(r, "agent_id")?.into(),
                    agent_kind: col_enum(r, "agent_kind")?,
                    from_value,
                    to_value,
                    gain_pct: gain_percent(opt.direction, from_value, to_value).unwrap_or(0.0),
                    at: col(r, "at")?,
                })
            })
            .collect()
    }

    // ---- spend ------------------------------------------------------------------------------------------------

    pub async fn record_spend(&self, input: RecordSpendInput) -> Result<SpendEvent> {
        record_spend(&mut *self.conn().await?, input, self.now()).await
    }

    /// The distinct `source` values of an optimization's spend rows that start with `prefix` (e.g. `ec2:`), by source.
    pub async fn spend_sources(
        &self,
        optimization_id: &OptimizationId,
        prefix: &str,
    ) -> Result<Vec<String>> {
        let escaped = prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let rows = sqlx::query(
            "SELECT DISTINCT source FROM spend_events WHERE optimization_id = ?1 AND source LIKE ?2 ESCAPE '\\' ORDER BY source",
        )
        .bind(optimization_id.as_str())
        .bind(format!("{escaped}%"))
        .fetch_all(&mut *self.conn().await?)
        .await
        .map_err(db)?;
        rows.iter().map(|r| col(r, "source")).collect()
    }

    pub async fn total_spend(&self, optimization_id: &OptimizationId) -> Result<f64> {
        let r = sqlx::query(
            "SELECT coalesce(sum(usd), 0.0) AS total FROM spend_events WHERE optimization_id = ?1",
        )
        .bind(optimization_id.as_str())
        .fetch_one(&mut *self.conn().await?)
        .await
        .map_err(db)?;
        col(&r, "total")
    }

    pub async fn spend_breakdown(
        &self,
        optimization_id: &OptimizationId,
    ) -> Result<SpendBreakdown> {
        spend_breakdown(&mut *self.conn().await?, optimization_id.as_str()).await
    }

    /// Append the corrections that bring the infrastructure categories to the billed `actuals`, atomically
    /// (see `ratchet_core::ledger::SpendLedger::apply_spend_corrections`).
    pub async fn apply_spend_corrections(
        &self,
        id: &OptimizationId,
        actuals: &std::collections::BTreeMap<SpendCategory, f64>,
        source: &str,
        epsilon_usd: f64,
        at: i64,
    ) -> Result<Vec<CostReconcileLine>> {
        let (id, actuals, source) = (id.clone(), actuals.clone(), source.to_owned());
        self.transaction(move |tx| {
            Box::pin(async move {
                let current = spend_breakdown(tx, id.as_str()).await?.by_category;
                let lines = ratchet_core::cost::plan_corrections(&current, &actuals, epsilon_usd);
                for l in lines.iter().filter(|l| l.written) {
                    let mut input =
                        RecordSpendInput::new(id.clone(), l.category, l.delta_usd, source.clone());
                    input.estimated = Some(false);
                    input.at = Some(at);
                    record_spend(tx, input, at).await?;
                }
                Ok(lines)
            })
        })
        .await
    }

    // ---- activity & agent events ------------------------------------------------------------------------------

    /// Merged, newest-first feed of verdicts and ratchet moves.
    pub async fn recent_activity(
        &self,
        limit: i64,
        optimization_id: Option<&OptimizationId>,
    ) -> Result<Vec<ActivityItem>> {
        if limit <= 0 {
            return Ok(Vec::new());
        }
        let mut conn = self.conn().await?;
        let opt = optimization_id.map(OptimizationId::as_str);

        let verdict_rows = sqlx::query(
            "SELECT c.*, a.kind AS agent_kind FROM candidates c INNER JOIN agents a ON a.id = c.agent_id \
             WHERE c.verdict <> 'pending' AND (?1 IS NULL OR c.optimization_id = ?1) \
             ORDER BY c.verdict_at DESC, c.id DESC LIMIT ?2",
        )
        .bind(opt)
        .bind(limit)
        .fetch_all(&mut *conn)
        .await
        .map_err(db)?;
        let mut items: Vec<ActivityItem> = Vec::new();
        for r in &verdict_rows {
            let c = rowmap::candidate(r)?;
            items.push(ActivityItem::Verdict {
                at: c.verdict_at.unwrap_or(c.submitted_at),
                optimization_id: c.optimization_id,
                candidate_id: c.id,
                agent_id: c.agent_id,
                agent_kind: col_enum(r, "agent_kind")?,
                sha: c.sha,
                message: c.message,
                verdict: c.verdict,
                reasons: c.reasons,
            });
        }

        let move_rows = sqlx::query(
            "SELECT m.id AS move_id, m.optimization_id, m.from_value, m.to_value, m.at, c.id AS candidate_id, c.agent_id, c.sha, \
             c.message, a.kind AS agent_kind FROM ratchet_moves m INNER JOIN candidates c ON c.id = m.candidate_id \
             INNER JOIN agents a ON a.id = c.agent_id WHERE (?1 IS NULL OR m.optimization_id = ?1) \
             ORDER BY m.at DESC, m.id DESC LIMIT ?2",
        )
        .bind(opt)
        .bind(limit)
        .fetch_all(&mut *conn)
        .await
        .map_err(db)?;
        for r in &move_rows {
            items.push(ActivityItem::RatchetMove {
                at: col(r, "at")?,
                optimization_id: col::<String>(r, "optimization_id")?.into(),
                candidate_id: col::<String>(r, "candidate_id")?.into(),
                agent_id: col::<String>(r, "agent_id")?.into(),
                agent_kind: col_enum(r, "agent_kind")?,
                sha: col(r, "sha")?,
                message: col(r, "message")?,
                from_value: col(r, "from_value")?,
                to_value: col(r, "to_value")?,
            });
        }

        // Newest first; on a tie a ratchet move precedes the verdict that caused it.
        items.sort_by(|a, b| {
            b.at().cmp(&a.at()).then_with(|| match (a, b) {
                (ActivityItem::RatchetMove { .. }, ActivityItem::Verdict { .. }) => {
                    std::cmp::Ordering::Less
                }
                (ActivityItem::Verdict { .. }, ActivityItem::RatchetMove { .. }) => {
                    std::cmp::Ordering::Greater
                }
                _ => std::cmp::Ordering::Equal,
            })
        });
        items.truncate(limit as usize);
        Ok(items)
    }

    pub async fn append_agent_event(
        &self,
        input: AppendAgentEventInput,
    ) -> Result<AgentEventRecord> {
        let mut conn = self.conn().await?;
        require_agent(&mut conn, input.agent_id.as_str()).await?;
        let payload = input.payload.as_ref().map(json_text).transpose()?;
        let row = sqlx::query("INSERT INTO agent_events (agent_id, at, type, payload) VALUES (?1, ?2, ?3, ?4) RETURNING *")
            .bind(input.agent_id.as_str())
            .bind(input.at.unwrap_or_else(|| self.now()))
            .bind(&input.event_type)
            .bind(payload)
            .fetch_one(&mut *conn)
            .await
            .map_err(db)?;
        rowmap::agent_event(&row)
    }

    /// Events with id > `since_id` (default: all), oldest first. Poll with the last seen id.
    pub async fn agent_events(
        &self,
        agent_id: &AgentId,
        since_id: i64,
        limit: i64,
    ) -> Result<Vec<AgentEventRecord>> {
        let rows = sqlx::query(
            "SELECT * FROM agent_events WHERE agent_id = ?1 AND id > ?2 ORDER BY id ASC LIMIT ?3",
        )
        .bind(agent_id.as_str())
        .bind(since_id)
        .bind(limit)
        .fetch_all(&mut *self.conn().await?)
        .await
        .map_err(db)?;
        rows.iter().map(rowmap::agent_event).collect()
    }

    // ---- engine_kv --------------------------------------------------------------------------------------------

    /// Engine bookkeeping that must survive a crash (stop mode, plateau counters, calibration, ...). Keys are
    /// namespaced `opt:<id>:...`.
    pub async fn kv_get(&self, key: &str) -> Result<Option<String>> {
        let r = sqlx::query("SELECT value FROM engine_kv WHERE key = ?1")
            .bind(key)
            .fetch_optional(&mut *self.conn().await?)
            .await
            .map_err(db)?;
        r.map(|r| col(&r, "value")).transpose()
    }

    pub async fn kv_set(&self, key: &str, value: &str) -> Result<()> {
        sqlx::query("INSERT INTO engine_kv (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value")
            .bind(key)
            .bind(value)
            .execute(&mut *self.conn().await?)
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn kv_delete(&self, key: &str) -> Result<()> {
        sqlx::query("DELETE FROM engine_kv WHERE key = ?1")
            .bind(key)
            .execute(&mut *self.conn().await?)
            .await
            .map_err(db)?;
        Ok(())
    }

    /// Every `(key, value)` whose key starts with `prefix`, by key.
    pub async fn kv_prefix(&self, prefix: &str) -> Result<Vec<(String, String)>> {
        let escaped = prefix
            .replace('\\', "\\\\")
            .replace('%', "\\%")
            .replace('_', "\\_");
        let rows = sqlx::query(
            "SELECT key, value FROM engine_kv WHERE key LIKE ?1 ESCAPE '\\' ORDER BY key",
        )
        .bind(format!("{escaped}%"))
        .fetch_all(&mut *self.conn().await?)
        .await
        .map_err(db)?;
        rows.iter()
            .map(|r| Ok((col(r, "key")?, col(r, "value")?)))
            .collect()
    }
}

// ---- operations on a connection (shared by the methods above and by transactions) -------------------------------

async fn get_optimization(conn: &mut SqliteConnection, id: &str) -> Result<Option<Optimization>> {
    let r = sqlx::query("SELECT * FROM optimizations WHERE id = ?1")
        .bind(id)
        .fetch_optional(conn)
        .await
        .map_err(db)?;
    r.as_ref().map(rowmap::optimization).transpose()
}

async fn require_optimization(conn: &mut SqliteConnection, id: &str) -> Result<Optimization> {
    get_optimization(conn, id)
        .await?
        .ok_or_else(|| LedgerError::not_found(format!("optimization {id} not found")))
}

async fn get_sandbox(conn: &mut SqliteConnection, id: &str) -> Result<Option<Sandbox>> {
    let r = sqlx::query("SELECT * FROM sandboxes WHERE id = ?1")
        .bind(id)
        .fetch_optional(conn)
        .await
        .map_err(db)?;
    r.as_ref().map(rowmap::sandbox).transpose()
}

async fn get_baseline(conn: &mut SqliteConnection, id: &str) -> Result<Option<Baseline>> {
    let r = sqlx::query("SELECT * FROM baselines WHERE optimization_id = ?1")
        .bind(id)
        .fetch_optional(conn)
        .await
        .map_err(db)?;
    r.as_ref().map(rowmap::baseline).transpose()
}

async fn get_agent(conn: &mut SqliteConnection, id: &str) -> Result<Option<Agent>> {
    let r = sqlx::query("SELECT * FROM agents WHERE id = ?1")
        .bind(id)
        .fetch_optional(conn)
        .await
        .map_err(db)?;
    r.as_ref().map(rowmap::agent).transpose()
}

async fn require_agent(conn: &mut SqliteConnection, id: &str) -> Result<Agent> {
    get_agent(conn, id)
        .await?
        .ok_or_else(|| LedgerError::not_found(format!("agent {id} not found")))
}

async fn list_agents(conn: &mut SqliteConnection, optimization_id: &str) -> Result<Vec<Agent>> {
    // Agents of one optimization share a timestamp; insertion order (rowid) keeps the order they were chosen in.
    let rows = sqlx::query(
        "SELECT * FROM agents WHERE optimization_id = ?1 ORDER BY created_at ASC, rowid ASC",
    )
    .bind(optimization_id)
    .fetch_all(conn)
    .await
    .map_err(db)?;
    rows.iter().map(rowmap::agent).collect()
}

async fn get_candidate(conn: &mut SqliteConnection, id: &str) -> Result<Option<Candidate>> {
    let r = sqlx::query("SELECT * FROM candidates WHERE id = ?1")
        .bind(id)
        .fetch_optional(conn)
        .await
        .map_err(db)?;
    r.as_ref().map(rowmap::candidate).transpose()
}

async fn require_candidate(conn: &mut SqliteConnection, id: &str) -> Result<Candidate> {
    get_candidate(conn, id)
        .await?
        .ok_or_else(|| LedgerError::not_found(format!("candidate {id} not found")))
}

async fn current_best(conn: &mut SqliteConnection, optimization_id: &str) -> Result<Option<f64>> {
    let last = sqlx::query(
        "SELECT to_value FROM ratchet_moves WHERE optimization_id = ?1 ORDER BY id DESC LIMIT 1",
    )
    .bind(optimization_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(db)?;
    if let Some(r) = last {
        return Ok(Some(col(&r, "to_value")?));
    }
    Ok(get_baseline(conn, optimization_id).await?.map(|b| b.value))
}

async fn record_spend(
    conn: &mut SqliteConnection,
    input: RecordSpendInput,
    now: i64,
) -> Result<SpendEvent> {
    assert_finite("usd", input.usd)?;
    require_optimization(conn, input.optimization_id.as_str()).await?;
    if let Some(agent_id) = &input.agent_id {
        let agent = require_agent(conn, agent_id.as_str()).await?;
        if agent.optimization_id != input.optimization_id {
            return Err(LedgerError::invalid(format!(
                "agent {agent_id} does not belong to optimization {}",
                input.optimization_id
            )));
        }
    }
    let row = sqlx::query(
        "INSERT INTO spend_events (optimization_id, agent_id, category, usd, estimated, source, at) \
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) RETURNING *",
    )
    .bind(input.optimization_id.as_str())
    .bind(input.agent_id.as_ref().map(AgentId::as_str))
    .bind(input.category.as_str())
    .bind(input.usd)
    .bind(i64::from(input.estimated.unwrap_or(true)))
    .bind(&input.source)
    .bind(input.at.unwrap_or(now))
    .fetch_one(&mut *conn)
    .await
    .map_err(db)?;
    rowmap::spend_event(&row)
}

async fn spend_breakdown(
    conn: &mut SqliteConnection,
    optimization_id: &str,
) -> Result<SpendBreakdown> {
    let rows = sqlx::query(
        "SELECT agent_id, category, estimated, sum(usd) AS total FROM spend_events WHERE optimization_id = ?1 \
         GROUP BY agent_id, category, estimated",
    )
    .bind(optimization_id)
    .fetch_all(&mut *conn)
    .await
    .map_err(db)?;

    let agents = list_agents(conn, optimization_id).await?;
    let mut out = SpendBreakdown {
        total: 0.0,
        estimated: 0.0,
        actual: 0.0,
        by_category: CategoryTotals::default(),
        by_agent: Vec::new(),
    };
    // keyed by agent id (None: optimization-level spend), in first-seen order
    let mut per_agent: Vec<(Option<String>, AgentSpend)> = Vec::new();
    for r in &rows {
        let agent_id: Option<String> = col(r, "agent_id")?;
        let category: SpendCategory = col_enum(r, "category")?;
        let estimated = col::<i64>(r, "estimated")? != 0;
        let total: f64 = col(r, "total")?;
        out.total += total;
        if estimated {
            out.estimated += total;
        } else {
            out.actual += total;
        }
        out.by_category.add(category, total);

        let pos = match per_agent.iter().position(|(k, _)| *k == agent_id) {
            Some(p) => p,
            None => {
                let info = agent_id
                    .as_ref()
                    .and_then(|id| agents.iter().find(|a| a.id.as_str() == id.as_str()));
                per_agent.push((
                    agent_id.clone(),
                    AgentSpend {
                        agent_id: agent_id.clone().map(AgentId::from),
                        kind: info.map(|a| a.kind),
                        branch: info.map(|a| a.branch.clone()),
                        total: 0.0,
                        by_category: CategoryTotals::default(),
                    },
                ));
                per_agent.len() - 1
            }
        };
        per_agent[pos].1.total += total;
        per_agent[pos].1.by_category.add(category, total);
    }
    // agents first (creation order), optimization-level spend last
    for a in &agents {
        if let Some(p) = per_agent
            .iter()
            .position(|(k, _)| k.as_deref() == Some(a.id.as_str()))
        {
            out.by_agent.push(per_agent[p].1.clone());
        }
    }
    if let Some(p) = per_agent.iter().position(|(k, _)| k.is_none()) {
        out.by_agent.push(per_agent[p].1.clone());
    }
    Ok(out)
}

async fn create_optimization(
    conn: &mut SqliteConnection,
    input: CreateOptimizationInput,
    now: i64,
) -> Result<Optimization> {
    assert_finite("minGain", input.min_gain)?;
    assert_finite("budgetUsd", input.budget_usd)?;
    if input.min_gain < 0.0 {
        return Err(LedgerError::invalid("minGain must be >= 0"));
    }
    if input.budget_usd < 0.0 {
        return Err(LedgerError::invalid("budgetUsd must be >= 0"));
    }
    let id = input.id.unwrap_or_else(OptimizationId::generate);
    let value = input.value.as_ref();
    let row = sqlx::query(
        "INSERT INTO optimizations (id, name, repo, base_ref, metric, direction, min_gain, budget_usd, time_limit_s, status, \
         created_at, config_yaml, pack, objective_name, evaluator_kind, value_usd_per_unit, value_period, value_basis, \
         value_unit, sandbox_id) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20) \
         RETURNING *",
    )
    .bind(id.as_str())
    .bind(&input.name)
    .bind(&input.repo)
    .bind(&input.base_ref)
    .bind(&input.metric)
    .bind(input.direction.as_str())
    .bind(input.min_gain)
    .bind(input.budget_usd)
    .bind(input.time_limit_s)
    .bind(input.status.unwrap_or(OptimizationStatus::Draft).as_str())
    .bind(input.created_at.unwrap_or(now))
    .bind(&input.config_yaml)
    .bind(&input.pack)
    .bind(&input.objective_name)
    .bind(input.evaluator_kind.unwrap_or(EvaluatorKind::Benchmark).as_str())
    .bind(value.map(|v| v.usd_per_unit))
    .bind(value.map(|v| v.period.as_str()))
    .bind(value.map(|v| v.basis.as_str()))
    .bind(value.and_then(|v| v.unit.clone()))
    .bind(input.sandbox_id.as_ref().map(SandboxId::as_str))
    .fetch_one(&mut *conn)
    .await
    .map_err(db)?;
    rowmap::optimization(&row)
}

async fn move_ratchet(
    tx: &mut SqliteConnection,
    opt_id: &OptimizationId,
    cand_id: &CandidateId,
    to_value: f64,
    at: i64,
) -> Result<RatchetMove> {
    let opt = require_optimization(tx, opt_id.as_str()).await?;
    let cand = require_candidate(tx, cand_id.as_str()).await?;
    if cand.optimization_id != *opt_id {
        return Err(LedgerError::invalid(format!(
            "candidate {cand_id} does not belong to optimization {opt_id}"
        )));
    }
    let Some(from) = current_best(tx, opt_id.as_str()).await? else {
        return Err(LedgerError::new(
            ratchet_core::LedgerErrorCode::NoBaseline,
            format!("optimization {opt_id} has no baseline yet"),
        ));
    };
    if !is_improvement(opt.direction, from, to_value) {
        return Err(LedgerError::new(
            ratchet_core::LedgerErrorCode::RatchetNotImproved,
            format!(
                "{} is not strictly better than current best {} ({})",
                ratchet_core::numfmt::js_number_to_string(to_value),
                ratchet_core::numfmt::js_number_to_string(from),
                opt.direction
            ),
        ));
    }
    let row = sqlx::query(
        "INSERT INTO ratchet_moves (optimization_id, candidate_id, from_value, to_value, at) \
         VALUES (?1, ?2, ?3, ?4, ?5) RETURNING *",
    )
    .bind(opt_id.as_str())
    .bind(cand_id.as_str())
    .bind(from)
    .bind(to_value)
    .bind(at)
    .fetch_one(&mut *tx)
    .await
    .map_err(db)?;
    rowmap::ratchet_move(&row)
}

/// The writes of one [`Ledger::atomically`] transaction, for callers that must commit several rows together (the
/// engine's poll, finalize and metering steps). Each method is the transactional form of the `Ledger` method of the
/// same name.
pub struct Tx<'c> {
    conn: &'c mut SqliteConnection,
    now: i64,
}

impl Tx<'_> {
    pub async fn create_optimization(
        &mut self,
        input: CreateOptimizationInput,
    ) -> Result<Optimization> {
        create_optimization(self.conn, input, self.now).await
    }

    pub async fn add_agent(&mut self, input: AddAgentInput) -> Result<Agent> {
        require_optimization(self.conn, input.optimization_id.as_str()).await?;
        let id = input.id.unwrap_or_else(AgentId::generate);
        let row = sqlx::query(
            "INSERT INTO agents (id, optimization_id, kind, branch, runner_instance_id, status, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7) RETURNING *",
        )
        .bind(id.as_str())
        .bind(input.optimization_id.as_str())
        .bind(input.kind.as_str())
        .bind(&input.branch)
        .bind(&input.runner_instance_id)
        .bind(input.status.unwrap_or(AgentStatus::Pending).as_str())
        .bind(input.created_at.unwrap_or(self.now))
        .fetch_one(&mut *self.conn)
        .await
        .map_err(db)?;
        rowmap::agent(&row)
    }

    /// Change the status only (the runner instance is kept).
    pub async fn set_agent_status(&mut self, id: &AgentId, status: AgentStatus) -> Result<Agent> {
        require_agent(self.conn, id.as_str()).await?;
        let row = sqlx::query("UPDATE agents SET status = ?1 WHERE id = ?2 RETURNING *")
            .bind(status.as_str())
            .bind(id.as_str())
            .fetch_one(&mut *self.conn)
            .await
            .map_err(db)?;
        rowmap::agent(&row)
    }

    pub async fn append_agent_event(
        &mut self,
        input: AppendAgentEventInput,
    ) -> Result<AgentEventRecord> {
        require_agent(self.conn, input.agent_id.as_str()).await?;
        let payload = input.payload.as_ref().map(json_text).transpose()?;
        let row = sqlx::query("INSERT INTO agent_events (agent_id, at, type, payload) VALUES (?1, ?2, ?3, ?4) RETURNING *")
            .bind(input.agent_id.as_str())
            .bind(input.at.unwrap_or(self.now))
            .bind(&input.event_type)
            .bind(payload)
            .fetch_one(&mut *self.conn)
            .await
            .map_err(db)?;
        rowmap::agent_event(&row)
    }

    pub async fn record_spend(&mut self, input: RecordSpendInput) -> Result<SpendEvent> {
        record_spend(self.conn, input, self.now).await
    }

    pub async fn set_verdict(
        &mut self,
        candidate_id: &CandidateId,
        verdict: CandidateVerdict,
        reasons: &[String],
        at: i64,
    ) -> Result<Candidate> {
        require_candidate(self.conn, candidate_id.as_str()).await?;
        let row = sqlx::query("UPDATE candidates SET verdict = ?1, reasons = ?2, verdict_at = ?3 WHERE id = ?4 RETURNING *")
            .bind(verdict.as_str())
            .bind(json_text(reasons)?)
            .bind((verdict != CandidateVerdict::Pending).then_some(at))
            .bind(candidate_id.as_str())
            .fetch_one(&mut *self.conn)
            .await
            .map_err(db)?;
        rowmap::candidate(&row)
    }

    pub async fn move_ratchet(
        &mut self,
        optimization_id: &OptimizationId,
        candidate_id: &CandidateId,
        to_value: f64,
        at: i64,
    ) -> Result<RatchetMove> {
        assert_finite("toValue", to_value)?;
        move_ratchet(self.conn, optimization_id, candidate_id, to_value, at).await
    }

    pub async fn kv_set(&mut self, key: &str, value: &str) -> Result<()> {
        sqlx::query("INSERT INTO engine_kv (key, value) VALUES (?1, ?2) ON CONFLICT(key) DO UPDATE SET value = excluded.value")
            .bind(key)
            .bind(value)
            .execute(&mut *self.conn)
            .await
            .map_err(db)?;
        Ok(())
    }

    pub async fn kv_delete(&mut self, key: &str) -> Result<()> {
        sqlx::query("DELETE FROM engine_kv WHERE key = ?1")
            .bind(key)
            .execute(&mut *self.conn)
            .await
            .map_err(db)?;
        Ok(())
    }
}
