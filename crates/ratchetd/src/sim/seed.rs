//! Seeded demo data (port of `packages/desktop/src/main/seed.ts`): one live run with 11 candidates, 5 verified
//! ratchet moves and two agents, two finished runs and a draft, all written through the normal ledger API.

use ratchet_core::config::GuardrailResult;
use ratchet_core::enums::{
    AgentKind, AgentStatus, BenchPhase, CandidateVerdict, Direction, OptimizationStatus,
    SpendCategory,
};
use ratchet_core::ids::{AgentId, OptimizationId};
use ratchet_core::ledger::Patch;
use ratchet_core::parse_config;
use ratchet_core::rows::BenchStats;
use ratchet_ledger::{
    AddAgentInput, AppendAgentEventInput, Ledger, RecordBaselineInput, RecordBenchResultInput,
    RecordSpendInput, SubmitCandidateInput,
};
use serde_json::json;

use super::data::{SeedValue, demo_config_yaml, simulated_stats};
use super::new_optimization_input;
use crate::service::{ServiceError, ServiceResult};
use crate::views::format::{MIN_MS, round_to};

/// Id of the seeded optimization that keeps "running" (the simulation resumes it).
pub const DEMO_RUNNING_ID: &str = "opt_demo_search";

struct Cand {
    /// Minutes after the optimization started.
    t: i64,
    /// Index into the spec's agents.
    agent: usize,
    value: f64,
    verdict: CandidateVerdict,
    message: &'static str,
    reason: &'static str,
    /// Hidden-trace value (verified / failed_confirm only).
    confirm: Option<f64>,
}

#[derive(Clone, Copy, PartialEq)]
enum SpecStatus {
    Running,
    Completed,
    Draft,
}

struct Spec {
    id: &'static str,
    name: &'static str,
    repo: &'static str,
    metric: &'static str,
    direction: Direction,
    min_gain: f64,
    baseline: f64,
    noise: f64,
    agents: &'static [AgentKind],
    status: SpecStatus,
    /// Minutes before `now` that the optimization started / ended.
    started_ago: i64,
    ended_ago: Option<i64>,
    spend_usd: f64,
    budget_usd: f64,
    /// The config's `objective.value`: the demo model (USD per ms of p95) only makes sense for the search demo.
    value: SeedValue,
    candidates: Vec<Cand>,
}

fn c(
    t: i64,
    agent: usize,
    value: f64,
    verdict: CandidateVerdict,
    confirm: Option<f64>,
    message: &'static str,
    reason: &'static str,
) -> Cand {
    Cand {
        t,
        agent,
        value,
        verdict,
        message,
        reason,
        confirm,
    }
}

fn specs() -> Vec<Spec> {
    use CandidateVerdict::{Blocked, FailedConfirm, Rejected, Verified};
    vec![
        Spec {
            id: DEMO_RUNNING_ID,
            name: "search-api p95 latency",
            repo: "acme/search-api",
            metric: "http_req_duration.p95",
            direction: Direction::Minimize,
            min_gain: 0.03,
            baseline: 182.0,
            noise: 3.1,
            agents: &[AgentKind::ClaudeCode, AgentKind::Codex],
            status: SpecStatus::Running,
            started_ago: 224,
            ended_ago: None,
            spend_usd: 42.6,
            budget_usd: 120.0,
            value: SeedValue::Default,
            candidates: vec![
                c(
                    14,
                    1,
                    179.5,
                    Rejected,
                    None,
                    "Inline hot-path helper in query parser",
                    "improvement 2.5 does not exceed required gain 5.46 (max of noise floor 3.1 and min_gain 5.46)",
                ),
                c(
                    31,
                    0,
                    168.0,
                    Verified,
                    Some(169.4),
                    "Drop the JSON deep-clone; build hits only for the returned page",
                    "bench improvement 14 exceeds required gain 5.46",
                ),
                c(
                    55,
                    1,
                    171.0,
                    Rejected,
                    None,
                    "Pool JSON encoder buffers",
                    "candidate 171 is not better than current best 168 (minimize)",
                ),
                c(
                    78,
                    1,
                    156.0,
                    Verified,
                    Some(158.2),
                    "Memoize parseFilter for repeated filter strings",
                    "bench improvement 12 exceeds required gain 5.04",
                ),
                c(
                    96,
                    0,
                    148.0,
                    FailedConfirm,
                    Some(160.1),
                    "Skip scoring for zero-weight terms",
                    "confirm run 160.1 does not beat current best 156 on the hidden trace (minimize)",
                ),
                c(
                    112,
                    0,
                    149.0,
                    Verified,
                    Some(150.7),
                    "Bounded top-k selection with a precomputed collation rank",
                    "bench improvement 7 exceeds required gain 4.68",
                ),
                c(
                    125,
                    1,
                    142.0,
                    Blocked,
                    None,
                    "Tighten bench warm-up window",
                    "protected path changed: bench/k6/search.js (matches bench/**)",
                ),
                c(
                    140,
                    0,
                    152.0,
                    Rejected,
                    None,
                    "Pre-size result arrays",
                    "candidate 152 is not better than current best 149 (minimize)",
                ),
                c(
                    171,
                    1,
                    138.0,
                    Verified,
                    Some(140.3),
                    "Category / brand / tag position lists for filter-only queries",
                    "bench improvement 11 exceeds required gain 4.47",
                ),
                c(
                    190,
                    0,
                    140.0,
                    Rejected,
                    None,
                    "Typed arrays in the scoring loop",
                    "candidate 140 is not better than current best 138 (minimize)",
                ),
                c(
                    214,
                    0,
                    131.0,
                    Verified,
                    Some(133.0),
                    "Binary-search the sorted vocabulary for prefix terms",
                    "bench improvement 7 exceeds required gain 4.14",
                ),
            ],
        },
        Spec {
            id: "opt_demo_ingest",
            name: "Ingest throughput",
            repo: "acme/ingest",
            metric: "throughput.events_per_s",
            direction: Direction::Maximize,
            min_gain: 0.03,
            baseline: 41.2,
            noise: 0.9,
            agents: &[AgentKind::Codex],
            status: SpecStatus::Completed,
            started_ago: 24 * 60 + 90,
            ended_ago: Some(120),
            spend_usd: 61.2,
            budget_usd: 100.0,
            value: SeedValue::Own {
                usd_per_unit: 400.0,
                unit: "k/s",
            },
            candidates: vec![
                c(
                    90,
                    0,
                    49.8,
                    Verified,
                    Some(49.1),
                    "Batch writes into 4 KiB segments",
                    "bench improvement 8.6 exceeds required gain 1.24",
                ),
                c(
                    170,
                    0,
                    50.4,
                    Rejected,
                    None,
                    "Larger socket buffers",
                    "improvement 0.6 does not exceed required gain 1.49",
                ),
                c(
                    260,
                    0,
                    58.9,
                    Verified,
                    Some(57.6),
                    "Vectorise the parser inner loop",
                    "bench improvement 9.1 exceeds required gain 1.49",
                ),
            ],
        },
        Spec {
            id: "opt_demo_edge",
            name: "Edge worker cold start",
            repo: "acme/edge-worker",
            metric: "cold_start.s",
            direction: Direction::Minimize,
            min_gain: 0.03,
            baseline: 2.41,
            noise: 0.05,
            agents: &[AgentKind::ClaudeCode, AgentKind::Codex],
            status: SpecStatus::Completed,
            // Finished about six weeks ago: the 30d KPI window compares against it.
            started_ago: 41 * 24 * 60,
            ended_ago: Some(40 * 24 * 60),
            spend_usd: 38.9,
            budget_usd: 60.0,
            value: SeedValue::Own {
                usd_per_unit: 3000.0,
                unit: "s",
            },
            candidates: vec![c(
                120,
                1,
                1.62,
                Verified,
                Some(1.66),
                "Lazy-load the template engine",
                "bench improvement 0.79 exceeds required gain 0.072",
            )],
        },
        Spec {
            id: "opt_demo_web",
            name: "Web render p99",
            repo: "acme/web",
            metric: "render.p99",
            direction: Direction::Minimize,
            min_gain: 0.03,
            baseline: 0.0,
            noise: 0.0,
            agents: &[],
            status: SpecStatus::Draft,
            started_ago: 72 * 60,
            ended_ago: None,
            spend_usd: 0.0,
            budget_usd: 50.0,
            value: SeedValue::None,
            candidates: vec![],
        },
    ]
}

fn around(v: f64) -> Vec<f64> {
    [0.0, 0.004, -0.003, 0.002, -0.005]
        .iter()
        .map(|d| round_to(v * (1.0 + d), 3))
        .collect()
}

fn hash(s: &str) -> i32 {
    s.chars()
        .fold(0i32, |h, c| h.wrapping_mul(31).wrapping_add(c as i32))
}

fn ledger_err(e: ratchet_core::LedgerError) -> ServiceError {
    ServiceError::from(e)
}

/// True when the ledger holds no optimization yet (so seeding is allowed).
pub async fn is_ledger_empty(ledger: &Ledger) -> ServiceResult<bool> {
    Ok(ledger.list_optimizations(None).await?.is_empty())
}

/// Seed a realistic set of optimizations. Call only when the ledger is empty.
pub async fn seed_demo(ledger: &Ledger, now: i64) -> ServiceResult<()> {
    for spec in specs() {
        seed_one(ledger, &spec, now).await?;
    }
    Ok(())
}

async fn seed_one(ledger: &Ledger, spec: &Spec, now: i64) -> ServiceResult<()> {
    let started = now - spec.started_ago * MIN_MS;
    let yaml = demo_config_yaml(spec.name, spec.metric, spec.direction.as_str(), spec.value);
    let cfg = parse_config(&yaml).map_err(|e| {
        ServiceError::Internal(format!("seed config for {} is invalid: {e:?}", spec.id))
    })?;
    let id = OptimizationId::new(spec.id);
    let mut input = new_optimization_input(
        spec.repo,
        "main",
        &cfg,
        &yaml,
        spec.budget_usd,
        8 * 3600,
        Some(spec.name.to_owned()),
    );
    input.id = Some(id.clone());
    input.min_gain = spec.min_gain;
    input.status = Some(OptimizationStatus::Draft);
    input.created_at = Some(started - 5 * MIN_MS);
    ledger
        .create_optimization(input)
        .await
        .map_err(ledger_err)?;
    if spec.status == SpecStatus::Draft {
        return Ok(());
    }

    ledger
        .set_optimization_status(&id, OptimizationStatus::Baselining, Some(started))
        .await?;
    let samples = around(spec.baseline);
    ledger
        .record_baseline(RecordBaselineInput {
            optimization_id: id.clone(),
            samples,
            noise_floor: spec.noise,
            stats: simulated_stats(spec.metric, spec.baseline, &[]),
            value: None,
            recorded_at: Some(started + 2 * MIN_MS),
        })
        .await?;
    ledger
        .set_optimization_status(&id, OptimizationStatus::Running, Some(started + 3 * MIN_MS))
        .await?;

    let mut agent_ids: Vec<AgentId> = Vec::new();
    for (i, kind) in spec.agents.iter().enumerate() {
        let mut a = AddAgentInput::new(
            id.clone(),
            *kind,
            format!("ratchet/{}/{}-{}", &spec.id[9..], kind, i + 1),
        );
        a.id = Some(AgentId::new(format!("{}_agt_{i}", spec.id)));
        a.runner_instance_id = Some(format!("i-0{}a{i}c9e1b7d", spec.id.len()));
        a.status = Some(AgentStatus::Running);
        a.created_at = Some(started);
        agent_ids.push(ledger.add_agent(a).await?.id);
    }
    for (i, a) in agent_ids.iter().enumerate() {
        ledger
            .append_agent_event(AppendAgentEventInput {
                agent_id: a.clone(),
                event_type: "status".into(),
                payload: Some(json!({"text": "Agent runner online; reading RATCHET.md"})),
                at: Some(started + (4 + i as i64) * MIN_MS),
            })
            .await?;
    }

    // spend weights -> scaled to the target total
    let parts = [
        (SpendCategory::Tokens, 0.55),
        (SpendCategory::BenchRunner, 0.3),
        (SpendCategory::AgentRunner, 0.09),
        (SpendCategory::Experiment, 0.05),
        (SpendCategory::StorageEgress, 0.01),
    ];
    let w_sum: f64 = parts.iter().map(|p| p.1).sum::<f64>() * (spec.candidates.len().max(1) as f64);
    let scale = spec.spend_usd / w_sum;

    let mut best = spec.baseline;
    for (n, cd) in spec.candidates.iter().enumerate() {
        let agent_id = agent_ids[cd.agent].clone();
        let at = started + cd.t * MIN_MS;
        let sha = format!(
            "{:x}a1b2c3d4e5f60718293a4b5c6d7e8f9012345678",
            hash(&format!("{}{n}", spec.id)).unsigned_abs()
        )[..40]
            .to_owned();
        let event = |t: &str, text: String, data: Option<serde_json::Value>, at: i64| {
            AppendAgentEventInput {
                agent_id: agent_id.clone(),
                event_type: t.into(),
                payload: Some(match data {
                    Some(d) => json!({"text": text, "data": d}),
                    None => json!({"text": text}),
                }),
                at: Some(at),
            }
        };
        ledger
            .append_agent_event(event("message", cd.message.into(), None, at - 4 * MIN_MS))
            .await?;
        ledger
            .append_agent_event(event(
                "tool",
                "edit src/search/index.ts".into(),
                None,
                at - 3 * MIN_MS,
            ))
            .await?;
        ledger
            .append_agent_event(event(
                "commit",
                format!("Ratchet-Candidate: {}", &sha[..7]),
                Some(json!({"sha": sha})),
                at - MIN_MS,
            ))
            .await?;
        let mut sub = SubmitCandidateInput::new(id.clone(), agent_id.clone(), sha.clone());
        sub.message = Some(cd.message.to_owned());
        sub.submitted_at = Some(at);
        let cand = ledger.submit_candidate(sub).await?;
        let blocked = cd.verdict == CandidateVerdict::Blocked;
        let guardrails = if blocked {
            vec![GuardrailResult {
                name: "protected paths".into(),
                passed: false,
                detail: Some(cd.reason.to_owned()),
            }]
        } else {
            vec![
                GuardrailResult {
                    name: "pnpm test:unit".into(),
                    passed: true,
                    detail: None,
                },
                GuardrailResult {
                    name: "error_rate < 0.001".into(),
                    passed: true,
                    detail: None,
                },
            ]
        };
        let stat = |v: f64| -> BenchStats { simulated_stats(spec.metric, v, &[]) };
        ledger
            .record_bench_result(RecordBenchResultInput {
                candidate_id: cand.id.clone(),
                phase: BenchPhase::Bench,
                samples: around(cd.value),
                stats: stat(cd.value),
                guardrails,
                started_at: at + 20_000,
                finished_at: at + 4 * MIN_MS,
            })
            .await?;
        if let Some(confirm) = cd.confirm {
            ledger
                .record_bench_result(RecordBenchResultInput {
                    candidate_id: cand.id.clone(),
                    phase: BenchPhase::Confirm,
                    samples: around(confirm),
                    stats: stat(confirm),
                    guardrails: vec![],
                    started_at: at + 4 * MIN_MS,
                    finished_at: at + 7 * MIN_MS,
                })
                .await?;
        }
        ledger
            .set_verdict(
                &cand.id,
                cd.verdict,
                &[cd.reason.to_owned()],
                Some(at + 7 * MIN_MS),
            )
            .await?;
        if cd.verdict == CandidateVerdict::Verified {
            ledger
                .move_ratchet(&id, &cand.id, cd.value, Some(at + 7 * MIN_MS))
                .await?;
            best = cd.value;
        }
        ledger
            .append_agent_event(event(
                "verdict",
                format!("{}: {}", cd.verdict, cd.reason),
                Some(json!({"sha": sha, "verdict": cd.verdict.as_str(), "value": cd.value, "best": best})),
                at + 7 * MIN_MS,
            ))
            .await?;
        for (cat, w) in parts {
            let mut s = RecordSpendInput::new(id.clone(), cat, round_to(w * scale, 4), "seed");
            s.agent_id = Some(agent_id.clone());
            s.at = Some(at + 5 * MIN_MS);
            ledger.record_spend(s).await?;
        }
    }

    if spec.status == SpecStatus::Completed {
        for a in &agent_ids {
            ledger
                .set_agent_status(a, AgentStatus::Done, Patch::Keep)
                .await?;
        }
        ledger
            .set_optimization_status(
                &id,
                OptimizationStatus::Completed,
                Some(now - spec.ended_ago.unwrap_or(0) * MIN_MS),
            )
            .await?;
    }
    Ok(())
}
