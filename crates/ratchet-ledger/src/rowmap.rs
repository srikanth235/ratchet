//! Decoding SQLite rows into the `ratchet-core` record types.

use std::str::FromStr;

use ratchet_core::config::GuardrailResult;
use ratchet_core::ledger::LedgerError;
use ratchet_core::rows::{
    Agent, AgentEventRecord, Baseline, BenchResult, BenchStats, Candidate, Optimization,
    RatchetMove, Sandbox, SpendEvent,
};
use serde::de::DeserializeOwned;
use sqlx::Row;
use sqlx::sqlite::SqliteRow;

pub(crate) type Result<T> = std::result::Result<T, LedgerError>;

/// A database failure as a [`LedgerError`] (the message is SQLite's, e.g. `CHECK constraint failed: ...`).
pub(crate) fn db(e: sqlx::Error) -> LedgerError {
    // `sqlx::Error::Database` displays as "error returned from database: (code: 275) <sqlite message>"
    LedgerError::database(e.to_string())
}

fn get<'r, T>(row: &'r SqliteRow, col: &str) -> Result<T>
where
    T: sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
{
    row.try_get(col).map_err(db)
}

fn get_enum<T: FromStr>(row: &SqliteRow, col: &str) -> Result<T> {
    let s: String = get(row, col)?;
    s.parse()
        .map_err(|_| LedgerError::database(format!("column {col} holds an unknown value {s:?}")))
}

fn get_opt_enum<T: FromStr>(row: &SqliteRow, col: &str) -> Result<Option<T>> {
    let s: Option<String> = get(row, col)?;
    s.map(|s| {
        s.parse().map_err(|_| {
            LedgerError::database(format!("column {col} holds an unknown value {s:?}"))
        })
    })
    .transpose()
}

fn get_json<T: DeserializeOwned>(row: &SqliteRow, col: &str) -> Result<T> {
    let s: String = get(row, col)?;
    serde_json::from_str(&s)
        .map_err(|e| LedgerError::database(format!("column {col} holds invalid JSON: {e}")))
}

fn get_opt_json<T: DeserializeOwned>(row: &SqliteRow, col: &str) -> Result<Option<T>> {
    let s: Option<String> = get(row, col)?;
    s.map(|s| {
        serde_json::from_str(&s)
            .map_err(|e| LedgerError::database(format!("column {col} holds invalid JSON: {e}")))
    })
    .transpose()
}

pub(crate) fn json_text<T: serde::Serialize + ?Sized>(v: &T) -> Result<String> {
    serde_json::to_string(v)
        .map_err(|e| LedgerError::invalid(format!("cannot store value as JSON: {e}")))
}

pub(crate) fn sandbox(r: &SqliteRow) -> Result<Sandbox> {
    Ok(Sandbox {
        id: get::<String>(r, "id")?.into(),
        provider: get_enum(r, "provider")?,
        region: get(r, "region")?,
        account_id: get(r, "account_id")?,
        status: get_enum(r, "status")?,
        outputs: get_opt_json(r, "outputs")?,
        created_at: get(r, "created_at")?,
        applied_at: get(r, "applied_at")?,
        destroyed_at: get(r, "destroyed_at")?,
    })
}

pub(crate) fn optimization(r: &SqliteRow) -> Result<Optimization> {
    Ok(Optimization {
        id: get::<String>(r, "id")?.into(),
        name: get(r, "name")?,
        repo: get(r, "repo")?,
        base_ref: get(r, "base_ref")?,
        metric: get(r, "metric")?,
        direction: get_enum(r, "direction")?,
        min_gain: get(r, "min_gain")?,
        budget_usd: get(r, "budget_usd")?,
        time_limit_s: get(r, "time_limit_s")?,
        status: get_enum(r, "status")?,
        created_at: get(r, "created_at")?,
        started_at: get(r, "started_at")?,
        ended_at: get(r, "ended_at")?,
        config_yaml: get(r, "config_yaml")?,
        pack: get(r, "pack")?,
        objective_name: get(r, "objective_name")?,
        evaluator_kind: get_enum(r, "evaluator_kind")?,
        value_usd_per_unit: get(r, "value_usd_per_unit")?,
        value_period: get_opt_enum(r, "value_period")?,
        value_basis: get_opt_enum(r, "value_basis")?,
        value_unit: get(r, "value_unit")?,
        sandbox_id: get::<Option<String>>(r, "sandbox_id")?.map(Into::into),
    })
}

pub(crate) fn baseline(r: &SqliteRow) -> Result<Baseline> {
    Ok(Baseline {
        optimization_id: get::<String>(r, "optimization_id")?.into(),
        samples: get_json(r, "samples")?,
        value: get(r, "value")?,
        noise_floor: get(r, "noise_floor")?,
        stats: get_json::<BenchStats>(r, "stats")?,
        recorded_at: get(r, "recorded_at")?,
    })
}

pub(crate) fn agent(r: &SqliteRow) -> Result<Agent> {
    Ok(Agent {
        id: get::<String>(r, "id")?.into(),
        optimization_id: get::<String>(r, "optimization_id")?.into(),
        kind: get_enum(r, "kind")?,
        branch: get(r, "branch")?,
        runner_instance_id: get(r, "runner_instance_id")?,
        status: get_enum(r, "status")?,
        created_at: get(r, "created_at")?,
    })
}

pub(crate) fn candidate(r: &SqliteRow) -> Result<Candidate> {
    Ok(Candidate {
        id: get::<String>(r, "id")?.into(),
        optimization_id: get::<String>(r, "optimization_id")?.into(),
        agent_id: get::<String>(r, "agent_id")?.into(),
        sha: get(r, "sha")?,
        message: get(r, "message")?,
        submitted_at: get(r, "submitted_at")?,
        verdict: get_enum(r, "verdict")?,
        verdict_at: get(r, "verdict_at")?,
        reasons: get_json(r, "reasons")?,
    })
}

pub(crate) fn bench_result(r: &SqliteRow) -> Result<BenchResult> {
    Ok(BenchResult {
        id: get(r, "id")?,
        candidate_id: get::<String>(r, "candidate_id")?.into(),
        phase: get_enum(r, "phase")?,
        samples: get_json(r, "samples")?,
        stats: get_json(r, "stats")?,
        guardrails: get_json::<Vec<GuardrailResult>>(r, "guardrails")?,
        started_at: get(r, "started_at")?,
        finished_at: get(r, "finished_at")?,
    })
}

pub(crate) fn ratchet_move(r: &SqliteRow) -> Result<RatchetMove> {
    Ok(RatchetMove {
        id: get(r, "id")?,
        optimization_id: get::<String>(r, "optimization_id")?.into(),
        candidate_id: get::<String>(r, "candidate_id")?.into(),
        from_value: get(r, "from_value")?,
        to_value: get(r, "to_value")?,
        at: get(r, "at")?,
    })
}

pub(crate) fn spend_event(r: &SqliteRow) -> Result<SpendEvent> {
    Ok(SpendEvent {
        id: get(r, "id")?,
        optimization_id: get::<String>(r, "optimization_id")?.into(),
        agent_id: get::<Option<String>>(r, "agent_id")?.map(Into::into),
        category: get_enum(r, "category")?,
        usd: get(r, "usd")?,
        estimated: get::<i64>(r, "estimated")? != 0,
        source: get(r, "source")?,
        at: get(r, "at")?,
    })
}

pub(crate) fn agent_event(r: &SqliteRow) -> Result<AgentEventRecord> {
    Ok(AgentEventRecord {
        id: get(r, "id")?,
        agent_id: get::<String>(r, "agent_id")?.into(),
        at: get(r, "at")?,
        event_type: get(r, "type")?,
        payload: get_opt_json(r, "payload")?,
    })
}

/// `get` for the store's aggregate queries.
pub(crate) fn col<'r, T>(r: &'r SqliteRow, name: &str) -> Result<T>
where
    T: sqlx::Decode<'r, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
{
    get(r, name)
}

pub(crate) fn col_enum<T: FromStr>(r: &SqliteRow, name: &str) -> Result<T> {
    get_enum(r, name)
}
