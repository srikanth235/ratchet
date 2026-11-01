//! Port of `packages/core/src/ledger/ledger.test.ts`.

mod common;

use common::*;
use ratchet_core::LedgerErrorCode;
use ratchet_core::config::GuardrailResult;
use ratchet_core::enums::{
    AgentKind, AgentStatus, BenchPhase, CandidateVerdict, Direction, EvaluatorKind,
    OptimizationStatus, SandboxProviderKind, SandboxStatus, SpendCategory, ValueBasis, ValuePeriod,
};
use ratchet_core::ids::{AgentId, CandidateId, OptimizationId, SandboxId};
use ratchet_core::ledger::{Patch, RecordSandboxInput, SandboxStatusPatch};
use ratchet_core::rows::{ActivityItem, BenchStats, SpendBreakdown};
use ratchet_ledger::{
    AddAgentInput, AppendAgentEventInput, Ledger, LedgerOptions, RecordBenchResultInput,
    RecordSpendInput, SubmitCandidateInput, ValueInput,
};
use serde_json::json;
use sqlx::Row;

async fn raw_error(l: &Ledger, sql: &'static str) -> String {
    sqlx::query(sql)
        .execute(l.pool())
        .await
        .expect_err("expected the statement to fail")
        .to_string()
}

// ---------------------------------------------------------------- openLedger

// "applies migrations and enables foreign keys"
#[tokio::test]
async fn applies_migrations_and_enables_foreign_keys() {
    let l = ledger().await;
    let mut tables: Vec<String> = sqlx::query(
        "select name from sqlite_master where type='table' and name not like '\\_sqlx%' escape '\\' and name not like 'sqlite_%' and name not like '%drizzle%'",
    )
    .fetch_all(l.pool())
    .await
    .unwrap()
    .iter()
    .map(|r| r.get::<String, _>("name"))
    .collect();
    tables.sort();
    assert_eq!(
        tables,
        [
            "agent_events",
            "agents",
            "baselines",
            "bench_results",
            "candidates",
            "engine_kv",
            "optimizations",
            "ratchet_moves",
            "sandboxes",
            "spend_events"
        ]
    );
    let fk: i64 = sqlx::query("pragma foreign_keys")
        .fetch_one(l.pool())
        .await
        .unwrap()
        .get(0);
    assert_eq!(fk, 1);
}

// "uses WAL for file databases and is idempotent on reopen"
#[tokio::test]
async fn uses_wal_for_file_databases_and_is_idempotent_on_reopen() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("ledger.db");
    let a = Ledger::open(&path, LedgerOptions::default()).await.unwrap();
    let mode: String = sqlx::query("pragma journal_mode")
        .fetch_one(a.pool())
        .await
        .unwrap()
        .get(0);
    assert_eq!(mode, "wal");
    let mut input = opt_input();
    input.id = Some("opt_x".into());
    input.repo = "r".into();
    let opt = a.create_optimization(input).await.unwrap();
    a.close().await;
    let b = Ledger::open(&path, LedgerOptions::default()).await.unwrap();
    assert_eq!(
        b.get_optimization(&opt.id).await.unwrap().unwrap().repo,
        "r"
    );
    b.close().await;
}

// "enforces foreign keys"
#[tokio::test]
async fn enforces_foreign_keys() {
    let l = ledger().await;
    let e = raw_error(&l, "insert into agents (id, optimization_id, kind, branch, status, created_at) values ('a','nope','codex','b','pending',1)").await;
    assert!(e.contains("FOREIGN KEY"), "{e}");
}

// ---------------------------------------------------------------- optimizations

// "creates and fetches an optimization with defaults"
#[tokio::test]
async fn creates_and_fetches_an_optimization_with_defaults() {
    let l = ledger().await;
    let mut input = opt_input();
    input.name = Some("search p95".into());
    let o = l.create_optimization(input).await.unwrap();
    assert!(o.id.as_str().starts_with("opt_"));
    assert_eq!(
        (
            o.status,
            o.started_at,
            o.ended_at,
            o.name.as_deref(),
            o.direction
        ),
        (
            OptimizationStatus::Draft,
            None,
            None,
            Some("search p95"),
            Direction::Minimize
        )
    );
    assert_eq!(l.get_optimization(&o.id).await.unwrap(), Some(o));
    assert_eq!(l.get_optimization(&"missing".into()).await.unwrap(), None);
}

// "validates input and rejects bad enums at the DB level"
#[tokio::test]
async fn validates_input_and_rejects_bad_enums_at_the_db_level() {
    let l = ledger().await;
    let mut bad = opt_input();
    bad.budget_usd = -1.0;
    assert_eq!(
        l.create_optimization(bad).await.unwrap_err().code,
        LedgerErrorCode::Invalid
    );
    let mut nan = opt_input();
    nan.min_gain = f64::NAN;
    assert!(msg(l.create_optimization(nan).await).contains("finite"));
    // the Rust enums cannot hold a bad direction: the CHECK constraint is what guards raw writes
    let e = raw_error(
        &l,
        "insert into optimizations (id, repo, base_ref, metric, direction, min_gain, budget_usd, time_limit_s, status, created_at, config_yaml, evaluator_kind) \
         values ('o','r','m','m','sideways',0.1,1,1,'draft',1,'x','benchmark')",
    )
    .await;
    assert!(e.contains("CHECK"), "{e}");
}

// "stamps started/ended timestamps on status changes"
#[tokio::test]
async fn stamps_started_and_ended_timestamps_on_status_changes() {
    let l = ledger().await;
    let o = mk_opt(&l).await;
    let running = l
        .set_optimization_status(&o.id, OptimizationStatus::Running, Some(5000))
        .await
        .unwrap();
    assert_eq!(
        (running.status, running.started_at, running.ended_at),
        (OptimizationStatus::Running, Some(5000), None)
    );
    assert_eq!(
        l.set_optimization_status(&o.id, OptimizationStatus::Running, Some(6000))
            .await
            .unwrap()
            .started_at,
        Some(5000)
    );
    let done = l
        .set_optimization_status(&o.id, OptimizationStatus::Completed, Some(9000))
        .await
        .unwrap();
    assert_eq!(
        (done.status, done.ended_at),
        (OptimizationStatus::Completed, Some(9000))
    );
    assert!(
        msg(l
            .set_optimization_status(&"missing".into(), OptimizationStatus::Running, None)
            .await)
        .contains("not found")
    );
}

// ---------------------------------------------------------------- baseline

// "records, reads and re-records the baseline before the ratchet moves"
#[tokio::test]
async fn records_reads_and_re_records_the_baseline_before_the_ratchet_moves() {
    let l = ledger().await;
    let o = mk_opt(&l).await;
    assert_eq!(l.get_baseline(&o.id).await.unwrap(), None);
    mk_baseline(&l, &o.id, 120.0).await;
    let b = l.get_baseline(&o.id).await.unwrap().unwrap();
    assert_eq!(
        (b.value, b.noise_floor, b.samples.clone()),
        (120.0, 0.02, vec![119.0, 120.0, 121.0])
    );
    assert_eq!(b.stats.value, 120.0);
    mk_baseline(&l, &o.id, 110.0).await;
    assert_eq!(l.get_baseline(&o.id).await.unwrap().unwrap().value, 110.0);
    assert_eq!(l.current_best(&o.id).await.unwrap(), Some(110.0));
}

// "refuses to change the baseline after a move"
#[tokio::test]
async fn refuses_to_change_the_baseline_after_a_move() {
    let l = ledger().await;
    let s = setup(&l).await;
    let c = mk_candidate(&l, &s.opt.id, &s.claude.id, "a1").await;
    l.move_ratchet(&s.opt.id, &c.id, 90.0, None).await.unwrap();
    let e = l
        .record_baseline(ratchet_ledger::RecordBaselineInput {
            optimization_id: s.opt.id.clone(),
            samples: vec![1.0],
            noise_floor: 0.0,
            stats: BenchStats::new(50.0),
            value: None,
            recorded_at: None,
        })
        .await;
    assert!(msg(e).contains("after the ratchet has moved"));
}

// "honours an explicit value override"
#[tokio::test]
async fn honours_an_explicit_value_override() {
    let l = ledger().await;
    let o = mk_opt(&l).await;
    let b = l
        .record_baseline(ratchet_ledger::RecordBaselineInput {
            optimization_id: o.id.clone(),
            samples: vec![1.0],
            noise_floor: 0.0,
            stats: BenchStats::new(1.0),
            value: Some(7.0),
            recorded_at: None,
        })
        .await
        .unwrap();
    assert_eq!(b.value, 7.0);
}

// ---------------------------------------------------------------- agents

// "lists agents created together in the order they were added, whatever their ids"
#[tokio::test]
async fn lists_agents_created_together_in_the_order_they_were_added() {
    let l = ledger().await;
    let o = mk_opt(&l).await;
    for id in ["agt-z", "agt-a", "agt-m"] {
        let mut a = AddAgentInput::new(o.id.clone(), AgentKind::Mock, format!("ratchet/{id}"));
        a.id = Some(id.into());
        a.created_at = Some(5);
        l.add_agent(a).await.unwrap();
    }
    let ids: Vec<String> = l
        .list_agents(&o.id)
        .await
        .unwrap()
        .into_iter()
        .map(|a| a.id.to_string())
        .collect();
    assert_eq!(ids, ["agt-z", "agt-a", "agt-m"]);
}

// "adds, lists and updates agents"
#[tokio::test]
async fn stores_a_grok_agent() {
    let l = ledger().await;
    let o = mk_opt(&l).await;
    let g = l
        .add_agent(AddAgentInput::new(
            o.id.clone(),
            AgentKind::Grok,
            "ratchet/x/grok",
        ))
        .await
        .unwrap();
    assert_eq!(g.kind, AgentKind::Grok);
    assert_eq!(l.list_agents(&o.id).await.unwrap()[0].kind, AgentKind::Grok);
}

#[tokio::test]
async fn adds_lists_and_updates_agents() {
    let l = ledger().await;
    let o = mk_opt(&l).await;
    let a = l
        .add_agent(AddAgentInput::new(
            o.id.clone(),
            AgentKind::ClaudeCode,
            "ratchet/x/claude",
        ))
        .await
        .unwrap();
    assert_eq!(
        (a.status, a.runner_instance_id.clone(), a.kind),
        (AgentStatus::Pending, None, AgentKind::ClaudeCode)
    );
    let mut with_runner = AddAgentInput::new(o.id.clone(), AgentKind::Codex, "ratchet/x/codex");
    with_runner.runner_instance_id = Some("i-1".into());
    let b = l.add_agent(with_runner).await.unwrap();
    let listed: Vec<AgentId> = l
        .list_agents(&o.id)
        .await
        .unwrap()
        .into_iter()
        .map(|x| x.id)
        .collect();
    assert_eq!(listed, [a.id.clone(), b.id.clone()]);
    let running = l
        .set_agent_status(&a.id, AgentStatus::Running, Patch::Set("i-9".into()))
        .await
        .unwrap();
    assert_eq!(
        (running.status, running.runner_instance_id.as_deref()),
        (AgentStatus::Running, Some("i-9"))
    );
    assert_eq!(
        l.set_agent_status(&a.id, AgentStatus::Paused, Patch::Keep)
            .await
            .unwrap()
            .runner_instance_id
            .as_deref(),
        Some("i-9")
    );
    assert_eq!(
        l.get_agent(&a.id).await.unwrap().unwrap().status,
        AgentStatus::Paused
    );
    assert!(
        msg(l
            .add_agent(AddAgentInput::new("nope", AgentKind::Codex, "b"))
            .await)
        .contains("not found")
    );
    let e = raw_error(&l, "insert into agents (id, optimization_id, kind, branch, status, created_at) select 'g', id, 'gemini', 'b', 'pending', 1 from optimizations limit 1").await;
    assert!(e.contains("CHECK"), "{e}");
    assert!(
        msg(l
            .set_agent_status(&"nope".into(), AgentStatus::Running, Patch::Keep)
            .await)
        .contains("not found")
    );
}

// ---------------------------------------------------------------- candidates, bench results and verdicts

// "submits a candidate and rejects foreign agents / duplicate shas"
#[tokio::test]
async fn submits_a_candidate_and_rejects_foreign_agents_and_duplicate_shas() {
    let l = ledger().await;
    let s = setup(&l).await;
    let other = mk_opt(&l).await;
    let c = mk_candidate(&l, &s.opt.id, &s.claude.id, "abc").await;
    assert_eq!(
        (
            c.verdict,
            c.verdict_at,
            c.reasons.clone(),
            c.message.as_str()
        ),
        (CandidateVerdict::Pending, None, vec![], "msg abc")
    );
    assert_eq!(l.get_candidate(&c.id).await.unwrap(), Some(c));
    assert!(
        msg(l
            .submit_candidate(SubmitCandidateInput::new(
                other.id.clone(),
                s.claude.id.clone(),
                "z"
            ))
            .await)
        .contains("does not belong")
    );
    assert!(
        msg(l
            .submit_candidate(SubmitCandidateInput::new(
                s.opt.id.clone(),
                s.claude.id.clone(),
                "abc"
            ))
            .await)
        .contains("UNIQUE")
    );
    assert!(
        msg(l
            .submit_candidate(SubmitCandidateInput::new(s.opt.id.clone(), "nope", "q"))
            .await)
        .contains("not found")
    );
}

// "records bench results and sets verdicts"
#[tokio::test]
async fn records_bench_results_and_sets_verdicts() {
    let l = ledger().await;
    let s = setup(&l).await;
    let c = mk_candidate(&l, &s.opt.id, &s.claude.id, "abc").await;
    let r = l
        .record_bench_result(RecordBenchResultInput {
            candidate_id: c.id.clone(),
            phase: BenchPhase::Bench,
            samples: vec![90.0, 91.0, 89.0],
            stats: BenchStats::new(90.0).with("stddev", 1.0),
            guardrails: vec![GuardrailResult {
                name: "pytest".into(),
                passed: true,
                detail: None,
            }],
            started_at: 10,
            finished_at: 20,
        })
        .await
        .unwrap();
    assert_eq!(r.phase, BenchPhase::Bench);
    assert_eq!(
        r.guardrails,
        vec![GuardrailResult {
            name: "pytest".into(),
            passed: true,
            detail: None
        }]
    );
    let no_guards = l
        .record_bench_result(RecordBenchResultInput {
            candidate_id: c.id.clone(),
            phase: BenchPhase::Confirm,
            samples: vec![92.0],
            stats: BenchStats::new(92.0),
            guardrails: vec![],
            started_at: 30,
            finished_at: 40,
        })
        .await
        .unwrap();
    assert_eq!(no_guards.guardrails, vec![]);
    let missing = RecordBenchResultInput {
        candidate_id: "nope".into(),
        phase: BenchPhase::Bench,
        samples: vec![],
        stats: BenchStats::new(1.0),
        guardrails: vec![],
        started_at: 1,
        finished_at: 2,
    };
    assert!(msg(l.record_bench_result(missing).await).contains("not found"));
    let e = raw_error(&l, "insert into bench_results (candidate_id, phase, samples, stats, guardrails, started_at, finished_at) select id, 'warmup', '[]', '{}', '[]', 1, 2 from candidates limit 1").await;
    assert!(e.contains("CHECK"), "{e}");

    let v = l
        .set_verdict(
            &c.id,
            CandidateVerdict::FailedConfirm,
            &["hidden trace regressed".to_owned()],
            Some(77),
        )
        .await
        .unwrap();
    assert_eq!(
        (v.verdict, v.reasons.clone(), v.verdict_at),
        (
            CandidateVerdict::FailedConfirm,
            vec!["hidden trace regressed".to_owned()],
            Some(77)
        )
    );
    assert_eq!(
        l.set_verdict(&c.id, CandidateVerdict::Pending, &[], None)
            .await
            .unwrap()
            .verdict_at,
        None
    );
    assert!(
        msg(l
            .set_verdict(&"nope".into(), CandidateVerdict::Verified, &[], None)
            .await)
        .contains("not found")
    );
    let e = raw_error(&l, "update candidates set verdict = 'great'").await;
    assert!(e.contains("CHECK"), "{e}");
}

// "candidatesWithResults joins agent, results, headline values and ratchet flag"
#[tokio::test]
async fn candidates_with_results_joins_agent_results_headline_values_and_ratchet_flag() {
    let l = ledger().await;
    let s = setup(&l).await;
    let mut i1 = SubmitCandidateInput::new(s.opt.id.clone(), s.claude.id.clone(), "s1");
    i1.submitted_at = Some(100);
    let c1 = l.submit_candidate(i1).await.unwrap();
    let mut i2 = SubmitCandidateInput::new(s.opt.id.clone(), s.codex.id.clone(), "s2");
    i2.submitted_at = Some(200);
    let c2 = l.submit_candidate(i2).await.unwrap();
    for (phase, v, a, b) in [
        (BenchPhase::Bench, 80.0, 1, 2),
        (BenchPhase::Confirm, 82.0, 3, 4),
    ] {
        l.record_bench_result(RecordBenchResultInput {
            candidate_id: c1.id.clone(),
            phase,
            samples: vec![v],
            stats: BenchStats::new(v),
            guardrails: vec![],
            started_at: a,
            finished_at: b,
        })
        .await
        .unwrap();
    }
    l.set_verdict(&c1.id, CandidateVerdict::Verified, &[], None)
        .await
        .unwrap();
    l.move_ratchet(&s.opt.id, &c1.id, 82.0, None).await.unwrap();
    let list = l.candidates_with_results(&s.opt.id).await.unwrap();
    assert_eq!(
        list.iter()
            .map(|c| c.candidate.sha.as_str())
            .collect::<Vec<_>>(),
        ["s1", "s2"]
    );
    let first = &list[0];
    assert_eq!(
        (
            first.agent_kind,
            first.bench_value,
            first.confirm_value,
            first.moved_ratchet,
            first.candidate.verdict
        ),
        (
            AgentKind::ClaudeCode,
            Some(80.0),
            Some(82.0),
            true,
            CandidateVerdict::Verified
        )
    );
    assert_eq!(
        first.results.iter().map(|r| r.phase).collect::<Vec<_>>(),
        [BenchPhase::Bench, BenchPhase::Confirm]
    );
    let second = &list[1];
    assert_eq!(
        (
            second.agent_kind,
            second.bench_value,
            second.confirm_value,
            second.moved_ratchet,
            second.results.len()
        ),
        (AgentKind::Codex, None, None, false, 0)
    );
    assert_eq!(c2.id, second.candidate.id);
    let fresh = mk_opt(&l).await;
    assert_eq!(l.candidates_with_results(&fresh.id).await.unwrap(), vec![]);
}

// ---------------------------------------------------------------- ratchet

// "moves strictly forward when minimizing"
#[tokio::test]
async fn moves_strictly_forward_when_minimizing() {
    let l = ledger().await;
    let s = setup(&l).await;
    let mut ids = Vec::new();
    for sha in ["a", "b", "c", "d"] {
        ids.push(mk_candidate(&l, &s.opt.id, &s.claude.id, sha).await.id);
    }
    assert_eq!(l.current_best(&s.opt.id).await.unwrap(), Some(100.0));
    let m1 = l
        .move_ratchet(&s.opt.id, &ids[0], 90.0, None)
        .await
        .unwrap();
    assert_eq!((m1.from_value, m1.to_value), (100.0, 90.0));
    assert!(
        msg(l.move_ratchet(&s.opt.id, &ids[1], 90.0, None).await).contains("not strictly better")
    ); // equal
    assert!(
        msg(l.move_ratchet(&s.opt.id, &ids[1], 95.0, None).await).contains("not strictly better")
    ); // worse
    assert_eq!(
        l.move_ratchet(&s.opt.id, &ids[1], 95.0, None)
            .await
            .unwrap_err()
            .code,
        LedgerErrorCode::RatchetNotImproved
    );
    let m3 = l
        .move_ratchet(&s.opt.id, &ids[2], 85.0, None)
        .await
        .unwrap();
    assert_eq!((m3.from_value, m3.to_value), (90.0, 85.0));
    assert!(
        l.move_ratchet(&s.opt.id, &ids[3], 85.0001, None)
            .await
            .is_err()
    );
    assert_eq!(l.current_best(&s.opt.id).await.unwrap(), Some(85.0));
    let hist: Vec<(f64, f64)> = l
        .ratchet_history(&s.opt.id)
        .await
        .unwrap()
        .iter()
        .map(|m| (m.from_value, m.to_value))
        .collect();
    assert_eq!(hist, [(100.0, 90.0), (90.0, 85.0)]);
}

// "moves strictly forward when maximizing"
#[tokio::test]
async fn moves_strictly_forward_when_maximizing() {
    let l = ledger().await;
    let mut input = opt_input();
    input.direction = Direction::Maximize;
    input.metric = "rps".into();
    let s = setup_with(&l, input).await;
    let c1 = mk_candidate(&l, &s.opt.id, &s.claude.id, "a").await;
    let c2 = mk_candidate(&l, &s.opt.id, &s.claude.id, "b").await;
    let c3 = mk_candidate(&l, &s.opt.id, &s.claude.id, "c").await;
    assert!(
        msg(l.move_ratchet(&s.opt.id, &c1.id, 90.0, None).await).contains("not strictly better")
    );
    assert!(
        msg(l.move_ratchet(&s.opt.id, &c1.id, 100.0, None).await).contains("not strictly better")
    );
    l.move_ratchet(&s.opt.id, &c1.id, 120.0, None)
        .await
        .unwrap();
    assert!(
        msg(l.move_ratchet(&s.opt.id, &c2.id, 110.0, None).await).contains("not strictly better")
    );
    let m = l
        .move_ratchet(&s.opt.id, &c3.id, 130.0, None)
        .await
        .unwrap();
    assert_eq!((m.from_value, m.to_value), (120.0, 130.0));
    assert_eq!(l.current_best(&s.opt.id).await.unwrap(), Some(130.0));
}

// "rejects moves without a baseline, with foreign/unknown candidates, non-finite values and repeats"
#[tokio::test]
async fn rejects_moves_without_a_baseline_foreign_candidates_non_finite_values_and_repeats() {
    let l = ledger().await;
    let o = mk_opt(&l).await;
    let a = l
        .add_agent(AddAgentInput::new(o.id.clone(), AgentKind::Codex, "b"))
        .await
        .unwrap();
    let c = mk_candidate(&l, &o.id, &a.id, "a").await;
    assert_eq!(l.current_best(&o.id).await.unwrap(), None);
    let e = l.move_ratchet(&o.id, &c.id, 1.0, None).await.unwrap_err();
    assert_eq!(e.code, LedgerErrorCode::NoBaseline);
    assert!(e.to_string().contains("no baseline"));
    mk_baseline(&l, &o.id, 100.0).await;
    let other = setup(&l).await;
    let foreign = mk_candidate(&l, &other.opt.id, &other.claude.id, "f").await;
    assert!(msg(l.move_ratchet(&o.id, &foreign.id, 50.0, None).await).contains("does not belong"));
    assert!(msg(l.move_ratchet(&o.id, &"nope".into(), 50.0, None).await).contains("not found"));
    assert!(msg(l.move_ratchet(&"nope".into(), &c.id, 50.0, None).await).contains("not found"));
    assert!(msg(l.move_ratchet(&o.id, &c.id, f64::NAN, None).await).contains("finite"));
    l.move_ratchet(&o.id, &c.id, 50.0, None).await.unwrap();
    assert!(msg(l.move_ratchet(&o.id, &c.id, 40.0, None).await).contains("UNIQUE")); // a candidate moves the ratchet once
    assert_eq!(l.current_best(&o.id).await.unwrap(), Some(50.0));
}

// "ratchetHistory carries candidate, agent and step gain"
#[tokio::test]
async fn ratchet_history_carries_candidate_agent_and_step_gain() {
    let l = ledger().await;
    let s = setup(&l).await;
    let c = mk_candidate(&l, &s.opt.id, &s.codex.id, "deadbeef").await;
    l.move_ratchet(&s.opt.id, &c.id, 80.0, Some(4242))
        .await
        .unwrap();
    let h = &l.ratchet_history(&s.opt.id).await.unwrap()[0];
    assert_eq!(
        (
            h.candidate_id.clone(),
            h.sha.as_str(),
            h.agent_id.clone(),
            h.agent_kind,
            h.at
        ),
        (
            c.id.clone(),
            "deadbeef",
            s.codex.id.clone(),
            AgentKind::Codex,
            4242
        )
    );
    assert!((h.gain_pct - 20.0).abs() < 1e-9);
    assert!(msg(l.ratchet_history(&"nope".into()).await).contains("not found"));
}

// ---------------------------------------------------------------- append-only enforcement

async fn seed_append_only(l: &Ledger) -> OptimizationId {
    let s = setup(l).await;
    let c = mk_candidate(l, &s.opt.id, &s.claude.id, "a").await;
    l.move_ratchet(&s.opt.id, &c.id, 90.0, None).await.unwrap();
    let mut spend = RecordSpendInput::new(
        s.opt.id.clone(),
        SpendCategory::Tokens,
        1.5,
        "claude-stream",
    );
    spend.agent_id = Some(s.claude.id.clone());
    l.record_spend(spend).await.unwrap();
    s.opt.id
}

// "blocks UPDATE and DELETE on ratchet_moves"
#[tokio::test]
async fn blocks_update_and_delete_on_ratchet_moves() {
    let l = ledger().await;
    seed_append_only(&l).await;
    assert!(
        raw_error(&l, "update ratchet_moves set to_value = 1")
            .await
            .contains("ratchet_moves is append-only")
    );
    assert!(
        raw_error(&l, "delete from ratchet_moves")
            .await
            .contains("ratchet_moves is append-only")
    );
    let n: i64 = sqlx::query("select count(*) n from ratchet_moves")
        .fetch_one(l.pool())
        .await
        .unwrap()
        .get("n");
    assert_eq!(n, 1);
}

// "blocks UPDATE and DELETE on spend_events"
#[tokio::test]
async fn blocks_update_and_delete_on_spend_events() {
    let l = ledger().await;
    seed_append_only(&l).await;
    assert!(
        raw_error(&l, "update spend_events set usd = 0")
            .await
            .contains("spend_events is append-only")
    );
    assert!(
        raw_error(&l, "delete from spend_events")
            .await
            .contains("spend_events is append-only")
    );
    let usd: f64 = sqlx::query("select usd from spend_events")
        .fetch_one(l.pool())
        .await
        .unwrap()
        .get("usd");
    assert_eq!(usd, 1.5);
}

// "still allows INSERT, and mutable tables remain mutable"
#[tokio::test]
async fn still_allows_insert_and_mutable_tables_remain_mutable() {
    let l = ledger().await;
    let opt = seed_append_only(&l).await;
    l.record_spend(RecordSpendInput::new(
        opt.clone(),
        SpendCategory::Experiment,
        -0.5,
        "correction",
    ))
    .await
    .unwrap();
    assert!((l.total_spend(&opt).await.unwrap() - 1.0).abs() < 1e-9);
    l.set_optimization_status(&opt, OptimizationStatus::Running, None)
        .await
        .unwrap();
}

// ---------------------------------------------------------------- spend

// "records spend with defaults and validates"
#[tokio::test]
async fn records_spend_with_defaults_and_validates() {
    let l = ledger().await;
    let s = setup(&l).await;
    let mut input =
        RecordSpendInput::new(s.opt.id.clone(), SpendCategory::AgentRunner, 0.25, "meter");
    input.agent_id = Some(s.claude.id.clone());
    let e = l.record_spend(input).await.unwrap();
    assert_eq!(
        (e.estimated, e.agent_id.clone(), e.usd),
        (true, Some(s.claude.id.clone()), 0.25)
    );
    let mut actual =
        RecordSpendInput::new(s.opt.id.clone(), SpendCategory::StorageEgress, 1.0, "ce");
    actual.estimated = Some(false);
    let e = l.record_spend(actual).await.unwrap();
    assert_eq!((e.estimated, e.agent_id), (false, None));
    assert!(
        msg(l
            .record_spend(RecordSpendInput::new(
                s.opt.id.clone(),
                SpendCategory::Tokens,
                f64::INFINITY,
                "x"
            ))
            .await)
        .contains("finite")
    );
    assert!(
        msg(l
            .record_spend(RecordSpendInput::new(
                "nope",
                SpendCategory::Tokens,
                1.0,
                "x"
            ))
            .await)
        .contains("not found")
    );
    let mut bad_agent = RecordSpendInput::new(s.opt.id.clone(), SpendCategory::Tokens, 1.0, "x");
    bad_agent.agent_id = Some("nope".into());
    assert!(msg(l.record_spend(bad_agent).await).contains("not found"));
    let e = raw_error(&l, "insert into spend_events (optimization_id, category, usd, estimated, source, at) select id, 'coffee', 1, 1, 'x', 1 from optimizations limit 1").await;
    assert!(e.contains("CHECK"), "{e}");
    let other = mk_opt(&l).await;
    let mut foreign = RecordSpendInput::new(other.id.clone(), SpendCategory::Tokens, 1.0, "x");
    foreign.agent_id = Some(s.claude.id.clone());
    assert!(msg(l.record_spend(foreign).await).contains("does not belong"));
}

// "aggregates totals and breakdowns by category and agent"
#[tokio::test]
async fn aggregates_totals_and_breakdowns_by_category_and_agent() {
    let l = ledger().await;
    let s = setup(&l).await;
    let other = mk_opt(&l).await;
    let rec = |agent: Option<AgentId>, category: SpendCategory, usd: f64, estimated: bool| {
        let l = l.clone();
        let opt = s.opt.id.clone();
        async move {
            let mut i = RecordSpendInput::new(opt, category, usd, "t");
            i.agent_id = agent;
            i.estimated = Some(estimated);
            l.record_spend(i).await.unwrap();
        }
    };
    rec(
        Some(s.claude.id.clone()),
        SpendCategory::AgentRunner,
        2.0,
        true,
    )
    .await;
    rec(
        Some(s.claude.id.clone()),
        SpendCategory::AgentRunner,
        1.0,
        true,
    )
    .await;
    rec(Some(s.claude.id.clone()), SpendCategory::Tokens, 4.0, true).await;
    rec(Some(s.codex.id.clone()), SpendCategory::Tokens, 3.0, false).await;
    rec(
        Some(s.codex.id.clone()),
        SpendCategory::Experiment,
        0.5,
        true,
    )
    .await;
    rec(None, SpendCategory::BenchRunner, 5.0, true).await;
    l.record_spend(RecordSpendInput::new(
        other.id.clone(),
        SpendCategory::Tokens,
        999.0,
        "t",
    ))
    .await
    .unwrap();

    assert!((l.total_spend(&s.opt.id).await.unwrap() - 15.5).abs() < 1e-9);
    assert_eq!(l.total_spend(&other.id).await.unwrap(), 999.0);
    assert_eq!(l.total_spend(&mk_opt(&l).await.id).await.unwrap(), 0.0);

    let b = l.spend_breakdown(&s.opt.id).await.unwrap();
    assert!(
        (b.total - 15.5).abs() < 1e-9
            && (b.estimated - 12.5).abs() < 1e-9
            && (b.actual - 3.0).abs() < 1e-9
    );
    let c = b.by_category;
    assert_eq!(
        (
            c.agent_runner,
            c.experiment,
            c.bench_runner,
            c.storage_egress,
            c.tokens
        ),
        (3.0, 0.5, 5.0, 0.0, 7.0)
    );
    let by_agent: Vec<(Option<AgentId>, Option<AgentKind>, f64)> = b
        .by_agent
        .iter()
        .map(|a| (a.agent_id.clone(), a.kind, a.total))
        .collect();
    assert_eq!(
        by_agent,
        [
            (Some(s.claude.id.clone()), Some(AgentKind::ClaudeCode), 7.0),
            (Some(s.codex.id.clone()), Some(AgentKind::Codex), 3.5),
            (None, None, 5.0),
        ]
    );
    let first = b.by_agent[0].by_category;
    assert_eq!(
        (first.agent_runner, first.tokens, first.experiment),
        (3.0, 4.0, 0.0)
    );

    let empty = l.spend_breakdown(&mk_opt(&l).await.id).await.unwrap();
    assert_eq!(
        (
            empty.total,
            empty.estimated,
            empty.actual,
            empty.by_agent.len()
        ),
        (0.0, 0.0, 0.0, 0)
    );
    let SpendBreakdown { by_category, .. } = empty;
    assert_eq!(by_category.total(), 0.0);
}

// ---------------------------------------------------------------- listOptimizations

// "returns the exact table shape with best value, gain, spend and agent kinds"
#[tokio::test]
async fn list_optimizations_returns_the_exact_table_shape() {
    let l = ledger().await;
    let mut input = opt_input();
    input.name = Some("search p95".into());
    let s = setup_with(&l, input).await;
    l.set_optimization_status(&s.opt.id, OptimizationStatus::Running, Some(2000))
        .await
        .unwrap();
    let c1 = mk_candidate(&l, &s.opt.id, &s.claude.id, "a").await;
    let c2 = mk_candidate(&l, &s.opt.id, &s.codex.id, "b").await;
    mk_candidate(&l, &s.opt.id, &s.codex.id, "c").await;
    l.move_ratchet(&s.opt.id, &c1.id, 90.0, None).await.unwrap();
    l.move_ratchet(&s.opt.id, &c2.id, 80.0, None).await.unwrap();
    let mut tokens = RecordSpendInput::new(s.opt.id.clone(), SpendCategory::Tokens, 12.5, "t");
    tokens.agent_id = Some(s.claude.id.clone());
    l.record_spend(tokens).await.unwrap();
    l.record_spend(RecordSpendInput::new(
        s.opt.id.clone(),
        SpendCategory::BenchRunner,
        2.5,
        "t",
    ))
    .await
    .unwrap();

    let rows = l.list_optimizations(None).await.unwrap();
    assert_eq!(rows.len(), 1);
    let row = serde_json::to_value(&rows[0]).unwrap();
    assert_eq!(
        row,
        json!({
            "id": s.opt.id, "name": "search p95", "repo": "acme/search", "metric": "http_req_duration.p95", "direction": "minimize",
            "status": "running", "budgetUsd": 100.0, "createdAt": s.opt.created_at, "startedAt": 2000, "endedAt": null,
            "baselineValue": 100.0, "bestValue": 80.0, "gainPct": 20.0, "spendUsd": 15.0,
            "agentKinds": ["claude-code", "codex"], "agentCount": 2, "candidateCount": 3, "verifiedWins": 2
        })
    );
}

// "computes gain for maximize, handles no baseline, empty and ordering/filtering"
#[tokio::test]
async fn list_optimizations_handles_maximize_no_baseline_ordering_and_filtering() {
    let l = ledger().await;
    assert_eq!(l.list_optimizations(None).await.unwrap(), vec![]);

    let mut fresh_input = opt_input();
    fresh_input.name = Some("fresh".into());
    let fresh = l.create_optimization(fresh_input).await.unwrap();
    let r = &l.list_optimizations(None).await.unwrap()[0];
    assert_eq!(
        (
            r.id.clone(),
            r.baseline_value,
            r.best_value,
            r.gain_pct,
            r.spend_usd,
            r.agent_kinds.clone(),
            r.agent_count,
            r.candidate_count,
            r.verified_wins
        ),
        (fresh.id.clone(), None, None, None, 0.0, vec![], 0, 0, 0)
    );

    let mut input = opt_input();
    input.direction = Direction::Maximize;
    input.metric = "rps".into();
    input.name = Some("throughput".into());
    let s = setup_with(&l, input).await;
    let c = mk_candidate(&l, &s.opt.id, &s.claude.id, "a").await;
    l.move_ratchet(&s.opt.id, &c.id, 150.0, None).await.unwrap();
    l.set_optimization_status(&s.opt.id, OptimizationStatus::Completed, Some(9))
        .await
        .unwrap();

    let all = l.list_optimizations(None).await.unwrap();
    assert_eq!(
        all.iter()
            .map(|o| o.name.as_deref().unwrap())
            .collect::<Vec<_>>(),
        ["throughput", "fresh"]
    ); // newest first
    assert_eq!(
        (
            all[0].best_value,
            all[0].gain_pct,
            all[0].agent_kinds.clone(),
            all[0].status
        ),
        (
            Some(150.0),
            Some(50.0),
            vec![AgentKind::ClaudeCode, AgentKind::Codex],
            OptimizationStatus::Completed
        )
    );

    // baseline only: best == baseline, gain 0
    let mut base_only = opt_input();
    base_only.name = Some("base-only".into());
    let b = l.create_optimization(base_only).await.unwrap();
    mk_baseline(&l, &b.id, 10.0).await;
    let row = l
        .list_optimizations(None)
        .await
        .unwrap()
        .into_iter()
        .find(|o| o.id == b.id)
        .unwrap();
    assert_eq!((row.best_value, row.gain_pct), (Some(10.0), Some(0.0)));

    let completed = l
        .list_optimizations(Some(&[OptimizationStatus::Completed]))
        .await
        .unwrap();
    assert_eq!(
        completed
            .iter()
            .map(|o| o.name.as_deref().unwrap())
            .collect::<Vec<_>>(),
        ["throughput"]
    );
    assert_eq!(
        l.list_optimizations(Some(&[
            OptimizationStatus::Draft,
            OptimizationStatus::Completed
        ]))
        .await
        .unwrap()
        .len(),
        3
    );
}

// ---------------------------------------------------------------- recentActivity

// "merges verdicts and moves newest-first with limit and filter"
#[tokio::test]
async fn recent_activity_merges_verdicts_and_moves_newest_first_with_limit_and_filter() {
    let l = ledger().await;
    let s = setup(&l).await;
    let other = setup(&l).await;
    let mk = |sha: &str, agent: &AgentId, message: Option<&str>| {
        let l = l.clone();
        let (opt, sha, agent, message) = (
            s.opt.id.clone(),
            sha.to_owned(),
            agent.clone(),
            message.map(str::to_owned),
        );
        async move {
            let mut i = SubmitCandidateInput::new(opt, agent, sha);
            i.message = message;
            l.submit_candidate(i).await.unwrap()
        }
    };
    let c1 = mk("a", &s.claude.id, Some("first")).await;
    let c2 = mk("b", &s.codex.id, Some("second")).await;
    let c3 = mk("c", &s.codex.id, None).await;
    let o1 = l
        .submit_candidate(SubmitCandidateInput::new(
            other.opt.id.clone(),
            other.claude.id.clone(),
            "o",
        ))
        .await
        .unwrap();

    l.set_verdict(&c1.id, CandidateVerdict::Verified, &[], Some(100))
        .await
        .unwrap();
    l.move_ratchet(&s.opt.id, &c1.id, 90.0, Some(100))
        .await
        .unwrap();
    l.set_verdict(
        &c2.id,
        CandidateVerdict::Rejected,
        &["no gain".to_owned()],
        Some(200),
    )
    .await
    .unwrap();
    l.set_verdict(
        &o1.id,
        CandidateVerdict::Blocked,
        &["tests failed".to_owned()],
        Some(250),
    )
    .await
    .unwrap();
    // c3 stays pending: must not appear

    let feed = l.recent_activity(10, None).await.unwrap();
    let shape: Vec<(&str, i64)> = feed
        .iter()
        .map(|i| {
            (
                if matches!(i, ActivityItem::Verdict { .. }) {
                    "verdict"
                } else {
                    "ratchet_move"
                },
                i.at(),
            )
        })
        .collect();
    assert_eq!(
        shape,
        [
            ("verdict", 250),
            ("verdict", 200),
            ("ratchet_move", 100),
            ("verdict", 100)
        ]
    );
    match &feed[1] {
        ActivityItem::Verdict {
            verdict,
            reasons,
            agent_kind,
            message,
            sha,
            ..
        } => {
            assert_eq!(
                (
                    *verdict,
                    reasons.clone(),
                    *agent_kind,
                    message.as_str(),
                    sha.as_str()
                ),
                (
                    CandidateVerdict::Rejected,
                    vec!["no gain".to_owned()],
                    AgentKind::Codex,
                    "second",
                    "b"
                )
            );
        }
        other => panic!("{other:?}"),
    }
    match &feed[2] {
        ActivityItem::RatchetMove {
            from_value,
            to_value,
            agent_kind,
            candidate_id,
            ..
        } => {
            assert_eq!(
                (*from_value, *to_value, *agent_kind, candidate_id.clone()),
                (100.0, 90.0, AgentKind::ClaudeCode, c1.id.clone())
            );
        }
        other => panic!("{other:?}"),
    }
    assert!(feed.iter().all(|i| *i.candidate_id() != c3.id));

    assert_eq!(l.recent_activity(2, None).await.unwrap().len(), 2);
    assert_eq!(l.recent_activity(0, None).await.unwrap(), vec![]);
    let scoped = l.recent_activity(10, Some(&s.opt.id)).await.unwrap();
    assert_eq!(scoped.len(), 3);
    assert!(scoped.iter().all(|i| *i.optimization_id() == s.opt.id));
}

// ---------------------------------------------------------------- agent events

// "appends and pages by id"
#[tokio::test]
async fn agent_events_append_and_page_by_id() {
    let l = ledger().await;
    let s = setup(&l).await;
    let ev = |agent: &AgentId, ty: &str, payload: Option<serde_json::Value>, at: Option<i64>| {
        AppendAgentEventInput {
            agent_id: agent.clone(),
            event_type: ty.into(),
            payload,
            at,
        }
    };
    let e1 = l
        .append_agent_event(ev(
            &s.claude.id,
            "text",
            Some(json!({"text": "hi"})),
            Some(1),
        ))
        .await
        .unwrap();
    let e2 = l
        .append_agent_event(ev(
            &s.claude.id,
            "tool_use",
            Some(json!({"name": "bash", "args": [1, 2]})),
            Some(2),
        ))
        .await
        .unwrap();
    l.append_agent_event(ev(&s.codex.id, "text", None, None))
        .await
        .unwrap();
    let e4 = l
        .append_agent_event(ev(&s.claude.id, "result", Some(json!(null)), None))
        .await
        .unwrap();

    assert_eq!(e1.payload, Some(json!({"text": "hi"})));
    let n = l
        .append_agent_event(ev(&s.claude.id, "n", None, None))
        .await
        .unwrap();
    assert_eq!(n.payload, None);
    let types: Vec<String> = l
        .agent_events(&s.claude.id, 0, 500)
        .await
        .unwrap()
        .into_iter()
        .map(|e| e.event_type)
        .collect();
    assert_eq!(types, ["text", "tool_use", "result", "n"]);
    let ids: Vec<i64> = l
        .agent_events(&s.claude.id, e1.id, 500)
        .await
        .unwrap()
        .iter()
        .map(|e| e.id)
        .collect();
    assert_eq!(ids, [e2.id, e4.id, e4.id + 1]);
    let ids: Vec<i64> = l
        .agent_events(&s.claude.id, e2.id, 1)
        .await
        .unwrap()
        .iter()
        .map(|e| e.id)
        .collect();
    assert_eq!(ids, [e4.id]);
    assert_eq!(
        l.agent_events(&s.claude.id, 10_000, 500).await.unwrap(),
        vec![]
    );
    assert_eq!(l.agent_events(&s.codex.id, 0, 500).await.unwrap().len(), 1);
    assert!(
        msg(l
            .append_agent_event(ev(&"nope".into(), "x", None, None))
            .await)
        .contains("not found")
    );
}

// ---------------------------------------------------------------- transaction

// "rolls back on error"
#[tokio::test]
async fn transaction_rolls_back_on_error() {
    let l = ledger().await;
    let r: Result<(), _> = l
        .transaction(|tx| {
            Box::pin(async move {
                sqlx::query(
                    "insert into optimizations (id, repo, base_ref, metric, direction, min_gain, budget_usd, time_limit_s, status, created_at, config_yaml, evaluator_kind) \
                     values ('o','r','m','m','minimize',0.1,1,1,'draft',1,'x','benchmark')",
                )
                .execute(&mut *tx)
                .await
                .map_err(|e| ratchet_core::LedgerError::database(e.to_string()))?;
                Err(ratchet_core::LedgerError::invalid("boom"))
            })
        })
        .await;
    assert_eq!(r.unwrap_err().to_string(), "boom");
    assert_eq!(l.list_optimizations(None).await.unwrap(), vec![]);
}

// ---------------------------------------------------------------- generic columns

// "stores pack, evaluator kind and the value model; defaults when absent"
#[tokio::test]
async fn stores_pack_evaluator_kind_and_the_value_model() {
    let l = ledger().await;
    let mut input = opt_input();
    input.pack = Some("forecasting".into());
    input.objective_name = Some("Weekly WAPE".into());
    input.evaluator_kind = Some(EvaluatorKind::Backtest);
    input.value = Some(ValueInput {
        usd_per_unit: 5000.0,
        period: ValuePeriod::Month,
        basis: ValueBasis::Measured,
        unit: Some("pt".into()),
    });
    let o = l.create_optimization(input).await.unwrap();
    assert_eq!(
        (
            o.pack.as_deref(),
            o.objective_name.as_deref(),
            o.evaluator_kind,
            o.value_usd_per_unit,
            o.value_period,
            o.value_basis,
            o.value_unit.as_deref(),
            o.sandbox_id.clone()
        ),
        (
            Some("forecasting"),
            Some("Weekly WAPE"),
            EvaluatorKind::Backtest,
            Some(5000.0),
            Some(ValuePeriod::Month),
            Some(ValueBasis::Measured),
            Some("pt"),
            None
        )
    );
    let d = mk_opt(&l).await;
    assert_eq!(
        (
            d.pack,
            d.evaluator_kind,
            d.value_usd_per_unit,
            d.value_period
        ),
        (None, EvaluatorKind::Benchmark, None, None)
    );
}

// "rejects bad evaluator kinds and non-positive values at the DB level"
#[tokio::test]
async fn rejects_non_positive_values_and_bad_columns_at_the_db_level() {
    let l = ledger().await;
    let mut input = opt_input();
    input.value = Some(ValueInput {
        usd_per_unit: -1.0,
        period: ValuePeriod::Year,
        basis: ValueBasis::Estimated,
        unit: None,
    });
    assert!(msg(l.create_optimization(input).await).contains("CHECK"));
    let e = raw_error(&l, "insert into optimizations (id, repo, base_ref, metric, direction, min_gain, budget_usd, time_limit_s, status, created_at, config_yaml, evaluator_kind) values ('o','r','m','m','minimize',0.1,1,1,'draft',1,'x','vibes')").await;
    assert!(e.contains("CHECK"), "{e}");
    let e = raw_error(&l, "insert into optimizations (id, repo, base_ref, metric, direction, min_gain, budget_usd, time_limit_s, status, created_at, config_yaml, evaluator_kind, value_usd_per_unit, value_period, value_basis) values ('o','r','m','m','minimize',0.1,1,1,'draft',1,'x','benchmark',1,'week','estimated')").await;
    assert!(e.contains("CHECK"), "{e}");
}

// ---------------------------------------------------------------- sandboxes

// "records idempotently, stamps apply/destroy and links optimizations"
#[tokio::test]
async fn sandboxes_record_idempotently_stamp_apply_destroy_and_link_optimizations() {
    let l = ledger().await;
    let mut rec = RecordSandboxInput::new("sbx-1", SandboxProviderKind::Aws, "us-east-1");
    rec.account_id = Some("123456789012".into());
    let s = l.record_sandbox(rec).await.unwrap();
    assert_eq!(
        (s.status, s.outputs.clone(), s.applied_at),
        (SandboxStatus::Planned, None, None)
    );
    assert_eq!(
        l.record_sandbox(RecordSandboxInput::new(
            "sbx-1",
            SandboxProviderKind::Mock,
            "x"
        ))
        .await
        .unwrap()
        .provider,
        SandboxProviderKind::Aws
    );
    let applied = l
        .set_sandbox_status(
            "sbx-1",
            SandboxStatus::Applied,
            SandboxStatusPatch {
                outputs: Patch::Set(json!({"artifactsBucket": "b"})),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        (applied.status, applied.outputs.clone()),
        (
            SandboxStatus::Applied,
            Some(json!({"artifactsBucket": "b"}))
        )
    );
    assert!(applied.applied_at.is_some());
    let mut input = opt_input();
    input.sandbox_id = Some("sbx-1".into());
    let o = l.create_optimization(input).await.unwrap();
    let ids: Vec<OptimizationId> = l
        .optimizations_in_sandbox("sbx-1")
        .await
        .unwrap()
        .into_iter()
        .map(|r| r.id)
        .collect();
    assert_eq!(ids, [o.id]);
    assert!(
        l.set_sandbox_status(
            "sbx-1",
            SandboxStatus::Destroyed,
            SandboxStatusPatch::default()
        )
        .await
        .unwrap()
        .destroyed_at
        .is_some()
    );
    assert_eq!(l.list_sandboxes().await.unwrap().len(), 1);
}

// "fills in the account on apply and clears outputs on destroy"
#[tokio::test]
async fn sandboxes_fill_in_the_account_on_apply_and_clear_outputs_on_destroy() {
    let l = ledger().await;
    l.record_sandbox(RecordSandboxInput::new(
        "sbx-2",
        SandboxProviderKind::Mock,
        "us-east-1",
    ))
    .await
    .unwrap();
    let applied = l
        .set_sandbox_status(
            "sbx-2",
            SandboxStatus::Applied,
            SandboxStatusPatch {
                outputs: Patch::Set(json!({"artifactsBucket": "b"})),
                account_id: Patch::Set("000000000000".into()),
                at: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(
        (applied.account_id.as_deref(), applied.outputs.clone()),
        (Some("000000000000"), Some(json!({"artifactsBucket": "b"})))
    );
    let destroying = l
        .set_sandbox_status(
            "sbx-2",
            SandboxStatus::Destroying,
            SandboxStatusPatch::default(),
        )
        .await
        .unwrap();
    assert_eq!(destroying.outputs, Some(json!({"artifactsBucket": "b"})));
    let destroyed = l
        .set_sandbox_status(
            "sbx-2",
            SandboxStatus::Destroyed,
            SandboxStatusPatch {
                outputs: Patch::Clear,
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert_eq!(
        (destroyed.outputs, destroyed.account_id.as_deref()),
        (None, Some("000000000000"))
    );
}

// "rejects unknown sandboxes and bad statuses"
#[tokio::test]
async fn sandboxes_reject_unknown_ids_and_bad_providers() {
    let l = ledger().await;
    let mut input = opt_input();
    input.sandbox_id = Some(SandboxId::new("nope"));
    assert!(msg(l.create_optimization(input).await).contains("FOREIGN KEY"));
    assert_eq!(
        l.set_sandbox_status(
            "nope",
            SandboxStatus::Applied,
            SandboxStatusPatch::default()
        )
        .await
        .unwrap_err()
        .code,
        LedgerErrorCode::NotFound
    );
    let e = raw_error(&l, "insert into sandboxes (id, provider, region, status, created_at) values ('s','gcp','x','planned',1)").await;
    assert!(e.contains("CHECK"), "{e}");
}

// ---------------------------------------------------------------- beyond the TypeScript cases

#[tokio::test]
async fn engine_kv_round_trips_and_lists_by_prefix() {
    let l = ledger().await;
    assert_eq!(l.kv_get("opt:1:stop_mode").await.unwrap(), None);
    l.kv_set("opt:1:stop_mode", "kill").await.unwrap();
    l.kv_set("opt:1:stop_mode", "graceful").await.unwrap();
    l.kv_set("opt:1:calibration", "{}").await.unwrap();
    l.kv_set("opt:10:calibration", "{}").await.unwrap();
    l.kv_set("opt_x:1", "no").await.unwrap(); // `_` is not a wildcard
    assert_eq!(
        l.kv_get("opt:1:stop_mode").await.unwrap().as_deref(),
        Some("graceful")
    );
    let keys: Vec<String> = l
        .kv_prefix("opt:1:")
        .await
        .unwrap()
        .into_iter()
        .map(|(k, _)| k)
        .collect();
    assert_eq!(keys, ["opt:1:calibration", "opt:1:stop_mode"]);
    assert_eq!(l.kv_prefix("opt_").await.unwrap().len(), 1);
    l.kv_delete("opt:1:calibration").await.unwrap();
    assert_eq!(l.kv_get("opt:1:calibration").await.unwrap(), None);
}

#[tokio::test]
async fn candidate_ids_and_money_survive_a_json_round_trip() {
    let l = ledger().await;
    let s = setup(&l).await;
    let c = mk_candidate(&l, &s.opt.id, &s.claude.id, "abc").await;
    let json = serde_json::to_value(&c).unwrap();
    assert_eq!(json["optimizationId"], json!(s.opt.id));
    assert_eq!(json["verdict"], "pending");
    let back: ratchet_core::rows::Candidate = serde_json::from_value(json).unwrap();
    assert_eq!(back, c);
    let _ = CandidateId::new("x");
}
