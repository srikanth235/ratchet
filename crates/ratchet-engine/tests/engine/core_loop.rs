//! `packages/engine/src/engine.test.ts`

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use ratchet_core::enums::CandidateVerdict;
use ratchet_core::ports::{
    AgentUsage, BenchLaunchPurpose, BenchLaunchSpec, CloudCompute, GitHost, InstanceState,
    NoticeLevel, PortError,
};
use ratchet_engine::{EngineErrorCode as C, ReportBuilder};

use crate::common::*;

fn code<T: std::fmt::Debug>(r: Result<T, ratchet_engine::EngineError>) -> C {
    r.expect_err("expected an error").code
}

// ---------------------------------------------------------------------------------------- createOptimization

#[tokio::test]
async fn reads_ratchet_yaml_via_the_git_host_snapshots_it_and_creates_agents_with_branches() {
    let h = Harness::new(Default::default()).await;
    let id = h
        .engine
        .create_optimization(CreateOptimizationInput {
            repo_url: h.git.remote_url(),
            base_ref: "main".into(),
            agents: vec![ClaudeCode, Codex],
            budget_usd: 50.0,
            time_limit_seconds: 3600,
            plateau_evals: 5,
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(id, "opt-test");
    let opt = h.ledger.get_optimization(&oid(&id)).await.unwrap().unwrap();
    assert_eq!(opt.status, OptimizationStatus::Draft);
    assert_eq!(opt.metric, "latency_ms");
    assert_eq!(opt.direction.to_string(), "minimize");
    assert_eq!(opt.min_gain, 0.03);
    assert_eq!(opt.budget_usd, 50.0);
    assert_eq!(opt.time_limit_s, 3600);
    assert!(opt.config_yaml.contains("confirm_trace"));
    let agents = h.ledger.list_agents(&oid(&id)).await.unwrap();
    let rows: Vec<_> = agents
        .iter()
        .map(|a| (a.id.to_string(), a.kind, a.branch.clone(), a.status))
        .collect();
    assert_eq!(
        rows,
        vec![
            (
                "opt-test-claude-code".to_owned(),
                ClaudeCode,
                "ratchet/opt-test/claude-code".to_owned(),
                AgentStatus::Pending
            ),
            (
                "opt-test-codex".to_owned(),
                Codex,
                "ratchet/opt-test/codex".to_owned(),
                AgentStatus::Pending
            ),
        ]
    );
    assert!(
        h.log.is_empty(),
        "nothing touched the cloud: {:?}",
        h.log.all()
    );
}

#[tokio::test]
async fn gives_two_agents_of_the_same_kind_distinct_branches() {
    let h = Harness::new(Default::default()).await;
    let id = h
        .engine
        .create_optimization(CreateOptimizationInput {
            agents: vec![ClaudeCode, ClaudeCode],
            budget_usd: 1.0,
            time_limit_seconds: 60,
            ..small_input()
        })
        .await
        .unwrap();
    let branches: Vec<_> = h
        .ledger
        .list_agents(&oid(&id))
        .await
        .unwrap()
        .into_iter()
        .map(|a| a.branch)
        .collect();
    assert_eq!(
        branches,
        [
            "ratchet/opt-test/claude-code",
            "ratchet/opt-test/claude-code-2"
        ]
    );
}

#[tokio::test]
async fn validates_ids_input_and_the_config() {
    let h = Harness::new(Default::default()).await;
    let base = small_input();
    for bad in ["Bad_ID", "has/slash", "-lead"] {
        let r = h
            .engine
            .create_optimization(CreateOptimizationInput {
                id: Some(bad.into()),
                ..base.clone()
            })
            .await;
        assert_eq!(code(r), C::InvalidId, "{bad}");
    }
    let cases: Vec<(CreateOptimizationInput, C)> = vec![
        (
            CreateOptimizationInput {
                agents: vec![],
                ..base.clone()
            },
            C::InvalidInput,
        ),
        (
            CreateOptimizationInput {
                budget_usd: 0.0,
                ..base.clone()
            },
            C::InvalidInput,
        ),
        (
            CreateOptimizationInput {
                plateau_evals: 0,
                ..base.clone()
            },
            C::InvalidInput,
        ),
        (
            CreateOptimizationInput {
                config_path: Some("../etc/passwd".into()),
                ..base.clone()
            },
            C::InvalidInput,
        ),
    ];
    for (input, expected) in cases {
        assert_eq!(code(h.engine.create_optimization(input).await), expected);
    }
    h.git
        .set_file("ratchet.yaml", "version: 0\nobjective: {}\n");
    assert_eq!(
        code(h.engine.create_optimization(base.clone()).await),
        C::InvalidConfig
    );
    h.git.set_file("ratchet.yaml", CONFIG_YAML);
    h.git.remove_file("RATCHET.md");
    let err = h
        .engine
        .create_optimization(base.clone())
        .await
        .unwrap_err();
    assert!(err.message.contains("RATCHET.md"), "{}", err.message);
    assert!(h.ledger.list_optimizations(None).await.unwrap().is_empty());
    // duplicate ids
    h.git.set_file("RATCHET.md", "brief");
    h.engine
        .create_optimization(CreateOptimizationInput {
            id: Some("dup-1".into()),
            ..base.clone()
        })
        .await
        .unwrap();
    let r = h
        .engine
        .create_optimization(CreateOptimizationInput {
            id: Some("dup-1".into()),
            ..base
        })
        .await;
    assert_eq!(code(r), C::InvalidInput);
}

#[tokio::test]
async fn rejects_malformed_ids_on_every_public_entry_point() {
    let h = Harness::new(Default::default()).await;
    assert_eq!(code(h.engine.start("NOPE").await), C::InvalidId);
    assert_eq!(code(h.engine.stop("a b", None).await), C::InvalidId);
    assert_eq!(code(h.engine.pause_agent("../x").await), C::InvalidId);
    assert_eq!(code(h.engine.teardown("").await), C::InvalidId);
    assert_eq!(code(h.engine.open_pull_request("UP").await), C::InvalidId);
    assert_eq!(code(h.engine.start("unknown-opt").await), C::NotFound);
    assert_eq!(
        code(h.engine.pause_agent("unknown-agent").await),
        C::NotFound
    );
}

// ---------------------------------------------------------------------------------------- start

#[tokio::test]
async fn start_runs_the_baseline_records_the_noise_floor_then_launches_each_agent_in_order() {
    let h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    let id = &b.id;

    let baseline = h.ledger.get_baseline(&oid(id)).await.unwrap().unwrap();
    assert_eq!(baseline.value, 100.0); // median
    assert_eq!(baseline.samples, [100.0, 102.0, 98.0]);
    assert!((baseline.noise_floor - 4.0).abs() < 1e-9); // 2 * stddev
    assert_eq!(baseline.stats.get("metric:peak_rss"), Some(500.0));
    let opt = h.ledger.get_optimization(&oid(id)).await.unwrap().unwrap();
    assert_eq!(opt.status, OptimizationStatus::Running);
    assert!(opt.started_at.is_some());
    let jobs = h.bench.jobs();
    assert_eq!(
        (
            phase_str(jobs[0].phase),
            jobs[0].repeats,
            jobs[0].sha.as_str()
        ),
        ("baseline", 3, "main-sha")
    );
    // the baseline on the hidden trace, so confirm compares like with like
    assert_eq!(phase_str(jobs[1].phase), "confirm");
    assert_eq!(jobs[1].trace.as_deref(), Some("s3://sealed/trace.jsonl"));

    for a in b.agents.values() {
        let row = h
            .ledger
            .get_agent(&a.as_str().into())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.status, AgentStatus::Running);
        assert!(row.runner_instance_id.unwrap().starts_with("i-"));
    }
    // per-agent order
    let claude = b.agent(ClaudeCode);
    let order: Vec<String> = h
        .log
        .all()
        .into_iter()
        .filter(|l| {
            l.ends_with(&claude) || l.contains(&format!(":{claude}:")) || l.contains("claude-code")
        })
        .map(|l| l.split(':').next().unwrap().to_owned())
        .collect();
    assert_eq!(
        order,
        [
            "compute.createAgentIdentity",
            "compute.putSecret",
            "compute.putSecret",
            "git.createBranch",
            "compute.launchRunner",
            "runners.start"
        ]
    );

    let started = h
        .runners
        .started
        .lock()
        .unwrap()
        .iter()
        .find(|(hd, _)| hd.agent_id.as_str() == claude)
        .cloned()
        .unwrap();
    let (handle, config) = started;
    assert_eq!(config.optimization_id.as_str(), id);
    assert_eq!(config.branch, "ratchet/opt-test/claude-code");
    assert_eq!(config.base_ref, "main");
    assert_eq!(config.baseline_value, 100.0);
    assert_eq!(config.budget_usd, 100.0);
    assert!(config.brief.contains("Make it faster"));
    assert_eq!(config.deadline_at, h.clock.now() + 8 * 3600 * 1000);
    let row = h
        .ledger
        .get_agent(&claude.as_str().into())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(Some(handle.instance_id), row.runner_instance_id);
}

#[tokio::test]
async fn never_stores_secret_values_in_the_ledger() {
    let h = Harness::new(Default::default()).await;
    begun(&h).await;
    assert_eq!(h.compute.secret_count(), 4);
    let kv = h.ledger.kv_prefix("").await.unwrap();
    for (k, val) in &kv {
        assert!(!val.contains("value-of-"), "{k} holds a secret value");
    }
    // the whole database, as the TypeScript test did with the serialized file: every column of every table
    use sqlx::{Column, Row};
    let tables = sqlx::query(
        "select name from sqlite_master where type = 'table' and name not like 'sqlite_%'",
    )
    .fetch_all(h.ledger.pool())
    .await
    .unwrap();
    assert!(tables.len() > 3, "the ledger has its tables");
    for table in tables {
        let name: String = table.get("name");
        let rows = sqlx::query(sqlx::AssertSqlSafe(format!("select * from \"{name}\"")))
            .fetch_all(h.ledger.pool())
            .await
            .unwrap();
        for row in rows {
            for (i, col) in row.columns().iter().enumerate() {
                if let Ok(Some(text)) = row.try_get::<Option<String>, _>(i) {
                    assert!(
                        !text.contains("value-of-"),
                        "{name}.{} holds a secret value",
                        col.name()
                    );
                }
            }
        }
    }
    assert!(
        kv.iter()
            .any(|(_, val)| val.contains("/ratchet/opt-test/opt-test-codex/OPENAI_API_KEY"))
    ); // refs are fine
}

#[tokio::test]
async fn fails_the_optimization_and_cleans_up_when_a_secret_is_unavailable() {
    let h = Harness::new(HarnessOptions {
        settings: ratchet_engine::EngineSettings {
            secrets: Some(Arc::new(|_k, name| {
                (name != "GITHUB_TOKEN").then(|| "v".to_owned())
            })),
            ..Default::default()
        },
        ..Default::default()
    })
    .await;
    assert_eq!(code(h.begin(begin_opts()).await), C::MissingSecret);
    assert_eq!(h.status("opt-test").await, OptimizationStatus::Failed);
    assert!(h.compute.live(None).is_empty());
    assert!(h.compute.identity("opt-test-claude-code").unwrap().revoked);
}

#[tokio::test]
async fn fails_when_the_baseline_breaks_its_own_command_guardrail() {
    let h = Harness::new(Default::default()).await;
    h.bench.set(
        "main-sha",
        "baseline",
        ScriptedBench::samples(&[10.0, 10.0, 10.0])
            .with_guardrails(vec![failed("pytest -q", None)]),
    );
    let id = h
        .engine
        .create_optimization(CreateOptimizationInput {
            budget_usd: 5.0,
            plateau_evals: 3,
            ..small_input()
        })
        .await
        .unwrap();
    let err = h.engine.start(&id).await.unwrap_err();
    assert!(
        err.message.contains("baseline fails its own guardrails"),
        "{}",
        err.message
    );
    assert_eq!(h.status(&id).await, OptimizationStatus::Failed);
    assert!(h.ledger.get_baseline(&oid(&id)).await.unwrap().is_none());
}

#[tokio::test]
async fn can_only_be_started_once() {
    let h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    assert_eq!(code(h.engine.start(&b.id).await), C::InvalidState);
}

#[tokio::test]
async fn a_kill_switch_during_the_baseline_wins_and_start_returns_quietly() {
    let h = Harness::new(Default::default()).await;
    h.bench.close_gate(); // baseline never finishes on its own
    h.bench.set(
        "main-sha",
        "baseline",
        ScriptedBench::samples(&[1.0, 1.0, 1.0]),
    );
    let id = h
        .engine
        .create_optimization(CreateOptimizationInput {
            budget_usd: 5.0,
            plateau_evals: 3,
            ..small_input()
        })
        .await
        .unwrap();
    let e = h.engine.clone();
    let id2 = id.clone();
    let starting = tokio::spawn(async move { e.start(&id2).await });
    while h.bench.job_count() == 0 {
        tokio::task::yield_now().await;
    }
    h.engine.stop(&id, Some("changed my mind")).await.unwrap();
    starting.await.unwrap().unwrap();
    assert_eq!(h.status(&id).await, OptimizationStatus::Stopped);
    assert!(h.compute.live(None).is_empty());
    assert!(h.log.starting_with("compute.launchRunner").is_empty());
}

// ---------------------------------------------------------------------------------------- happy path

fn sent_verdicts(h: &Harness, agent: &str) -> Vec<(String, CandidateVerdict)> {
    h.runners
        .sent_to(agent)
        .into_iter()
        .filter_map(|m| match m {
            RunnerMessage::Verdict { sha, verdict, .. } => Some((sha, verdict)),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn baseline_two_agents_verified_rejected_blocked_and_failed_confirm_candidates_monotonic_ratchet_pr()
 {
    let mut h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    let id = b.id.clone();

    h.candidate(
        CC,
        "sha-c1",
        Cand::new()
            .bench(&[90.0, 91.0, 89.0])
            .confirm(&[88.0, 89.0, 90.0]),
    ); // verified 100 -> 90
    h.candidate(
        CC,
        "sha-c2",
        Cand::new()
            .bench(&[80.0, 81.0, 79.0])
            .confirm(&[95.0, 96.0, 97.0]),
    ); // bench win, fails the hidden trace
    h.candidate(
        CC,
        "sha-c3",
        Cand::new().bench(&[60.0, 60.0, 60.0]).guardrail_fail(),
    ); // guardrail failed
    h.candidate(CC, "sha-c4", Cand::new().bench(&[69.5, 70.0, 70.5])); // not better than 70
    h.candidate(
        CX,
        "sha-x1",
        Cand::new().files(&["src/a.ts", "bench/k6/search.js"]),
    ); // protected path, never benched
    h.candidate(CX, "sha-x2", Cand::new().bench(&[89.5, 90.0, 90.5])); // equal to the best: rejected
    h.candidate(
        CX,
        "sha-x3",
        Cand::new()
            .bench(&[70.0, 70.0, 71.0])
            .confirm(&[72.0, 72.0, 73.0]),
    ); // verified 90 -> 70

    h.until(async || all_decided(&h, &id, 7).await, 60).await;

    assert_eq!(
        h.verdicts(&id).await,
        v(&[
            ("sha-c1", "verified"),
            ("sha-c2", "failed_confirm"),
            ("sha-c3", "blocked"),
            ("sha-c4", "rejected"),
            ("sha-x1", "blocked"),
            ("sha-x2", "rejected"),
            ("sha-x3", "verified"),
        ])
    );
    let rows = candidates(&h, &id).await;
    let by = |sha: &str| rows.iter().find(|c| c.candidate.sha == sha).unwrap();
    assert!(
        by("sha-x1").candidate.reasons[0]
            .contains("protected path changed: bench/k6/search.js (matches bench/**)")
    );
    assert!(by("sha-x1").results.is_empty()); // blocked without spending a bench run
    assert!(by("sha-c3").candidate.reasons[0].contains("guardrail failed: pytest -q"));
    assert!(
        by("sha-c2")
            .candidate
            .reasons
            .join(" ")
            .contains("does not beat current best")
    );

    // ratchet only moved forward
    let hist = h.ledger.ratchet_history(&oid(&id)).await.unwrap();
    assert_eq!(
        hist.iter()
            .map(|m| (m.sha.as_str(), m.from_value, m.to_value))
            .collect::<Vec<_>>(),
        [("sha-c1", 100.0, 90.0), ("sha-x3", 90.0, 70.0)]
    );
    assert_eq!(h.ledger.current_best(&oid(&id)).await.unwrap(), Some(70.0));
    let phases = |sha: &str| {
        by(sha)
            .results
            .iter()
            .map(|r| r.phase.to_string())
            .collect::<Vec<_>>()
    };
    assert_eq!(phases("sha-c1"), ["bench", "confirm"]);
    assert_eq!(phases("sha-c2"), ["bench", "confirm"]);
    assert_eq!(phases("sha-x2"), ["bench"]);

    // one bench job at a time, round-robin between agents, confirm right after its bench
    let names = h.bench.job_names();
    assert_eq!(
        names[2..],
        [
            "sha-c1:bench",
            "sha-c1:confirm",
            "sha-x2:bench",
            "sha-c2:bench",
            "sha-c2:confirm",
            "sha-x3:bench",
            "sha-x3:confirm",
            "sha-c3:bench",
            "sha-c4:bench",
        ]
    );
    let jobs = h.bench.jobs();
    assert_eq!(
        jobs.iter()
            .find(|j| j.sha == "sha-c1" && phase_str(j.phase) == "confirm")
            .unwrap()
            .trace
            .as_deref(),
        Some("s3://sealed/trace.jsonl")
    );
    let last = h
        .bench
        .baseline_args
        .lock()
        .unwrap()
        .last()
        .cloned()
        .unwrap()
        .unwrap();
    assert_eq!(
        last.metrics,
        [
            ("objective".to_owned(), 100.0),
            ("peak_rss".to_owned(), 500.0)
        ]
        .into()
    );

    // verdicts reach the submitting agent; ratchet moves reach everyone else
    let (claude, codex) = (b.agent(ClaudeCode), b.agent(Codex));
    let to_claude = h.runners.sent_to(&claude);
    let to_codex = h.runners.sent_to(&codex);
    assert!(to_claude.iter().any(|m| matches!(m, RunnerMessage::Verdict { sha, verdict: CandidateVerdict::Verified, current_best, .. } if sha == "sha-c1" && *current_best == 90.0)));
    assert!(to_claude.contains(&RunnerMessage::Ratchet {
        sha: "sha-x3".into(),
        branch: CX.into(),
        value: 70.0
    }));
    assert!(to_codex.contains(&RunnerMessage::Ratchet {
        sha: "sha-c1".into(),
        branch: CC.into(),
        value: 90.0
    }));
    // the winner gets its verdict, not a ratchet announcement about itself
    assert!(
        !to_codex
            .iter()
            .any(|m| matches!(m, RunnerMessage::Ratchet { sha, .. } if sha == "sha-x3"))
    );
    assert!(
        !to_claude
            .iter()
            .any(|m| matches!(m, RunnerMessage::Ratchet { sha, .. } if sha == "sha-c1"))
    );
    assert!(!sent_verdicts(&h, &codex).iter().any(|(s, _)| s == "sha-c1"));
    assert!(sent_verdicts(&h, &codex).contains(&("sha-x1".to_owned(), CandidateVerdict::Blocked)));
    // a rejection is private to its author
    assert!(
        !sent_verdicts(&h, &claude)
            .iter()
            .any(|(s, _)| s == "sha-x2")
    );

    // events
    let events = h.events();
    let moves: Vec<(f64, f64)> = events
        .iter()
        .filter_map(|e| match e {
            EngineEvent::Ratchet { from, to, .. } => Some((*from, *to)),
            _ => None,
        })
        .collect();
    assert_eq!(moves, [(100.0, 90.0), (90.0, 70.0)]);
    let cand_events = |want: CandidateVerdict| {
        events
            .iter()
            .filter(|e| matches!(e, EngineEvent::Candidate { verdict, .. } if *verdict == want))
            .count()
    };
    assert_eq!(cand_events(CandidateVerdict::Pending), 7);
    assert_eq!(cand_events(CandidateVerdict::Verified), 2);
    assert!(types(&events).contains(&"spend"));
    // verdicts are in the per-agent log too
    let log = h
        .ledger
        .agent_events(&claude.as_str().into(), 0, 1000)
        .await
        .unwrap();
    assert_eq!(log.iter().filter(|e| e.event_type == "verdict").count(), 4);

    // end the run and open the PR
    assert_eq!(code(h.engine.open_pull_request(&id).await), C::InvalidState);
    h.engine.stop(&id, Some("done")).await.unwrap();
    let pr = h.engine.open_pull_request(&id).await.unwrap();
    assert_eq!(pr.url, "https://example.test/pr/1");
    let pr0 = h.git.pr(0);
    assert_eq!(
        (pr0.base.as_str(), pr0.head.as_str()),
        ("main", "ratchet/opt-test/result")
    );
    assert_eq!(pr0.shas, ["sha-c1", "sha-x3"]);
    assert!(
        pr0.title
            .contains("latency_ms 100 -> 70 (30% lower, 2 verified)"),
        "{}",
        pr0.title
    );
    assert!(pr0.body.contains("Verified wins"));
    assert!(pr0.body.contains("sha-c1"));
    assert_eq!(h.engine.open_pull_request(&id).await.unwrap().url, pr.url); // idempotent
    assert_eq!(h.git.pr_count(), 1);

    h.engine.teardown(&id).await.unwrap();
    assert_eq!(h.status(&id).await, OptimizationStatus::Completed);
    assert!(h.compute.live(None).is_empty());
}

struct Custom;

#[async_trait::async_trait]
impl ReportBuilder for Custom {
    async fn build(
        &self,
        _l: &ratchet_ledger::Ledger,
        id: &OptimizationId,
    ) -> Result<String, String> {
        Ok(format!("custom report for {id}"))
    }
}

#[tokio::test]
async fn uses_a_custom_report_builder_for_the_pr_body() {
    let h = Harness::new(HarnessOptions {
        settings: ratchet_engine::EngineSettings {
            report_builder: Some(Arc::new(Custom)),
            ..Default::default()
        },
        ..Default::default()
    })
    .await;
    let b = begun(&h).await;
    h.candidate(CX, "sha-1", win(80.0));
    h.until(
        async || h.verdicts(&b.id).await.get("sha-1").map(String::as_str) == Some("verified"),
        60,
    )
    .await;
    h.engine.stop(&b.id, None).await.unwrap();
    h.engine.open_pull_request(&b.id).await.unwrap();
    assert_eq!(h.git.pr(0).body, "custom report for opt-test");
    assert_eq!(
        h.artifacts.text("engine/opt-test/report.md").as_deref(),
        Some("custom report for opt-test")
    );
}

#[tokio::test]
async fn refuses_to_open_a_pr_without_a_verified_candidate() {
    let h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    h.engine.stop(&b.id, None).await.unwrap();
    assert_eq!(code(h.engine.open_pull_request(&b.id).await), C::NoVerified);
}

#[tokio::test]
async fn keeps_the_hidden_trace_baseline_separate_from_the_bench_scale() {
    // Bench scale ~100 ms but the hidden trace runs at ~200 ms: a confirm of 190 beats 200, not 100.
    let h = Harness::new(Default::default()).await;
    let b = h
        .begin(BeginOptions {
            confirm_baseline: Some(vec![200.0; 3]),
            ..Default::default()
        })
        .await
        .unwrap();
    h.candidate(
        CX,
        "sha-1",
        Cand::new().bench(&[80.0; 3]).confirm(&[190.0; 3]),
    );
    h.until(
        async || h.verdicts(&b.id).await.get("sha-1").map(String::as_str) == Some("verified"),
        60,
    )
    .await;
}

#[tokio::test]
async fn retries_bench_infrastructure_failures_then_rejects_after_the_attempt_limit() {
    let h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    h.candidate(CX, "sha-flaky", Cand::new());
    h.bench.set(
        "sha-flaky",
        "bench",
        ScriptedBench::samples(&[80.0; 3]).failing(1),
    );
    h.bench
        .set("sha-flaky", "confirm", ScriptedBench::samples(&[80.0; 3]));
    h.candidate(CX, "sha-dead", Cand::new());
    h.bench.set(
        "sha-dead",
        "bench",
        ScriptedBench::samples(&[1.0; 3]).failing(99),
    );
    h.until(async || all_decided(&h, &b.id, 2).await, 60).await;
    assert_eq!(
        h.verdicts(&b.id).await,
        v(&[("sha-flaky", "verified"), ("sha-dead", "rejected")])
    );
    let rows = candidates(&h, &b.id).await;
    let dead = rows.iter().find(|c| c.candidate.sha == "sha-dead").unwrap();
    assert!(dead.candidate.reasons[0].contains("bench infrastructure error after 2 attempts"));
}

#[tokio::test]
async fn a_guardrail_that_fails_before_any_bench_run_blocks_the_candidate_instead_of_wedging_the_queue()
 {
    let h = Harness::new(Default::default()).await;
    let b = h
        .begin(BeginOptions {
            agents: Some(vec![Codex]),
            ..Default::default()
        })
        .await
        .unwrap();
    h.git.push(CX, "sha-brk", &["src/search.ts"]);
    h.bench.set(
        "sha-brk",
        "bench",
        ScriptedBench::samples(&[])
            .with_guardrails(vec![failed("pnpm test:unit", Some("exited with status 1"))]),
    );
    h.candidate(CX, "sha-next", win(70.0));
    h.until(async || all_decided(&h, &b.id, 2).await, 20).await;
    assert_eq!(
        h.verdicts(&b.id).await,
        v(&[("sha-brk", "blocked"), ("sha-next", "verified")])
    );
    let rows = candidates(&h, &b.id).await;
    let brk = rows.iter().find(|c| c.candidate.sha == "sha-brk").unwrap();
    assert!(
        brk.candidate.reasons[0]
            .contains("guardrail failed: pnpm test:unit (exited with status 1)")
    );
    assert_eq!(
        h.bench.jobs().iter().filter(|j| j.sha == "sha-brk").count(),
        1
    ); // not retried
}

#[tokio::test]
async fn an_outcome_with_neither_samples_nor_a_failed_guardrail_counts_as_a_failed_attempt() {
    let h = Harness::new(Default::default()).await;
    let b = h
        .begin(BeginOptions {
            agents: Some(vec![Codex]),
            ..Default::default()
        })
        .await
        .unwrap();
    h.git.push(CX, "sha-empty", &["src/search.ts"]);
    h.bench
        .set("sha-empty", "bench", ScriptedBench::samples(&[]));
    h.until(async || all_decided(&h, &b.id, 1).await, 20).await;
    assert_eq!(h.verdicts(&b.id).await, v(&[("sha-empty", "rejected")]));
    assert_eq!(
        h.bench
            .jobs()
            .iter()
            .filter(|j| j.sha == "sha-empty")
            .count(),
        2
    );
}

#[tokio::test]
async fn retries_a_candidate_whose_changed_files_cannot_be_read_yet_without_losing_order() {
    let h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    h.candidate(CX, "sha-a", win(90.0));
    h.candidate(CX, "sha-b", win(80.0));
    h.git
        .fail_changed_files
        .lock()
        .unwrap()
        .insert("sha-a".into());
    h.cycle(2).await;
    assert!(candidates(&h, &b.id).await.is_empty());
    h.git.fail_changed_files.lock().unwrap().clear();
    h.until(async || all_decided(&h, &b.id, 2).await, 60).await;
    let hist = h.ledger.ratchet_history(&oid(&b.id)).await.unwrap();
    assert_eq!(
        hist.iter().map(|m| m.sha.as_str()).collect::<Vec<_>>(),
        ["sha-a", "sha-b"]
    );
}

#[tokio::test]
async fn ignores_duplicate_candidates_the_git_host_relists() {
    let h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    h.candidate(CX, "sha-a", win(90.0));
    h.cycle(6).await;
    h.git.ignore_since.store(true, Ordering::SeqCst); // lists everything, ignoring `since_sha`
    h.cycle(3).await;
    assert_eq!(candidates(&h, &b.id).await.len(), 1);
}

// ---------------------------------------------------------------------------------------- cost and stop rules

fn approx(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6, "{a} != {b}");
}

#[tokio::test]
async fn meters_instances_x_price_by_category_with_estimated_true_from_a_per_instance_watermark() {
    let h = Harness::new(HarnessOptions {
        hourly: Some(3.6),
        ..Default::default()
    })
    .await; // $0.001 per second
    let b = begun(&h).await;
    h.compute.launch_experiment(&b.id, &b.agent(Codex));
    h.cycle(1).await; // +60 s
    let br = h.ledger.spend_breakdown(&oid(&b.id)).await.unwrap();
    approx(br.by_category.agent_runner, 2.0 * 60.0 * 0.001);
    approx(br.by_category.experiment, 60.0 * 0.001);
    assert_eq!(br.actual, 0.0);
    let codex = br
        .by_agent
        .iter()
        .find(|a| a.agent_id.as_ref().map(|x| x.as_str()) == Some(b.agent(Codex).as_str()))
        .unwrap();
    approx(codex.by_category.experiment, 0.06);

    // no double counting when nothing time-wise happened
    h.engine.tick().await.unwrap();
    approx(h.ledger.total_spend(&oid(&b.id)).await.unwrap(), br.total);
    h.cycle(1).await;
    approx(
        h.ledger
            .spend_breakdown(&oid(&b.id))
            .await
            .unwrap()
            .by_category
            .experiment,
        0.12,
    );
}

#[tokio::test]
async fn counts_bench_instances_as_bench_runner_spend() {
    let h = Harness::new(HarnessOptions {
        hourly: Some(3.6),
        ..Default::default()
    })
    .await;
    let b = begun(&h).await;
    h.compute
        .launch_bench_instance(&BenchLaunchSpec {
            optimization_id: oid(&b.id),
            instance_type: "c7i.2xlarge".into(),
            image_ref: "x".into(),
            purpose: BenchLaunchPurpose::Bench,
        })
        .await
        .unwrap();
    h.cycle(1).await;
    approx(
        h.ledger
            .spend_breakdown(&oid(&b.id))
            .await
            .unwrap()
            .by_category
            .bench_runner,
        0.06,
    );
}

#[tokio::test]
async fn records_token_usage_from_runner_events_as_spend_category_tokens() {
    let h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    let claude = b.agent(ClaudeCode);
    h.runners.emit(
        &claude,
        vec![
            ev(AgentEventType::Usage).with_usage(AgentUsage {
                input_tokens: 1_000_000,
                output_tokens: 100_000,
                cost_usd: None,
            }), // 3 + 1.5
            ev(AgentEventType::Usage).with_usage(AgentUsage {
                input_tokens: 0,
                output_tokens: 0,
                cost_usd: Some(0.25),
            }),
        ],
    );
    h.cycle(1).await;
    let br = h.ledger.spend_breakdown(&oid(&b.id)).await.unwrap();
    approx(br.by_category.tokens, 4.75);
    let mine = br
        .by_agent
        .iter()
        .find(|a| a.agent_id.as_ref().map(|x| x.as_str()) == Some(claude.as_str()))
        .unwrap();
    approx(mine.by_category.tokens, 4.75);
    h.cycle(1).await;
    approx(
        h.ledger
            .spend_breakdown(&oid(&b.id))
            .await
            .unwrap()
            .by_category
            .tokens,
        4.75,
    ); // not re-counted
}

#[tokio::test]
async fn warns_at_80_percent_and_trips_the_kill_switch_at_100_percent_in_the_right_order() {
    let mut h = Harness::new(HarnessOptions {
        hourly: Some(10.0),
        ..Default::default()
    })
    .await; // two runners: $0.333 per tick
    let b = h
        .begin(BeginOptions {
            budget_usd: Some(1.2),
            ..Default::default()
        })
        .await
        .unwrap();
    let id = &b.id;
    let mark = h.log.len();
    let warnings = |h: &mut Harness| {
        h.events()
            .iter()
            .filter(|e| {
                matches!(
                    e,
                    EngineEvent::Notice {
                        level: NoticeLevel::Warning,
                        ..
                    }
                )
            })
            .count()
    };

    h.cycle(3).await; // $1.0 = 83%
    assert_eq!(h.status(id).await, OptimizationStatus::Running);
    assert_eq!(warnings(&mut h), 1);
    h.cycle(1).await; // $1.333 >= $1.2
    assert_eq!(h.status(id).await, OptimizationStatus::Stopped);
    assert!(
        h.engine
            .stop_reason(id)
            .await
            .unwrap()
            .unwrap()
            .contains("budget exhausted")
    );
    assert_eq!(warnings(&mut h), 1);

    let after = h.log.since(mark);
    let idx = |p: &str| after.iter().position(|l| l.starts_with(p)).unwrap();
    let last_idx = |p: &str| after.iter().rposition(|l| l.starts_with(p)).unwrap();
    let revokes: Vec<_> = after
        .iter()
        .filter(|l| l.starts_with("compute.revoke"))
        .cloned()
        .collect();
    assert_eq!(
        revokes,
        [
            format!("compute.revoke:{}", b.agent(ClaudeCode)),
            format!("compute.revoke:{}", b.agent(Codex))
        ]
    );
    assert!(last_idx("compute.revoke") < idx("runners.stop"));
    assert!(last_idx("runners.stop") < idx("compute.terminate"));
    assert!(h.compute.live(None).is_empty());
    let statuses: Vec<_> = h
        .ledger
        .list_agents(&oid(id))
        .await
        .unwrap()
        .into_iter()
        .map(|a| a.status)
        .collect();
    assert_eq!(statuses, [AgentStatus::Stopped, AgentStatus::Stopped]);
    assert!(h.compute.identity(&b.agent(Codex)).unwrap().revoked);
    let spend = h.ledger.total_spend(&oid(id)).await.unwrap();
    assert!(spend >= 1.2);
    assert!(spend - 1.2 <= (2.0 * 10.0 * TICK_MS as f64) / 3_600_000.0 + 1e-9); // at most one interval over
    // no more cost accrues afterwards
    h.cycle(3).await;
    approx(h.ledger.total_spend(&oid(id)).await.unwrap(), spend);
}

#[tokio::test]
async fn a_token_spend_overrun_also_trips_the_kill_switch() {
    let h = Harness::new(Default::default()).await;
    let b = h
        .begin(BeginOptions {
            budget_usd: Some(2.0),
            ..Default::default()
        })
        .await
        .unwrap();
    h.runners.emit(
        &b.agent(Codex),
        vec![ev(AgentEventType::Usage).with_usage(AgentUsage {
            input_tokens: 0,
            output_tokens: 0,
            cost_usd: Some(5.0),
        })],
    );
    h.cycle(1).await;
    assert_eq!(h.status(&b.id).await, OptimizationStatus::Stopped);
    assert!(h.compute.live(None).is_empty());
}

#[tokio::test]
async fn stops_gracefully_at_the_deadline_runners_stop_queued_candidates_are_still_evaluated_then_teardown()
 {
    let h = Harness::new(Default::default()).await;
    let b = h
        .begin(BeginOptions {
            time_limit_seconds: Some(10 * 60),
            ..Default::default()
        })
        .await
        .unwrap();
    let id = &b.id;
    h.cycle(8).await;
    h.candidate(CX, "sha-late", win(80.0));
    h.cycle(1).await; // submitted + bench started + collected into the job
    assert_eq!(h.status(id).await, OptimizationStatus::Running);
    h.cycle(1).await; // deadline reached: bench result processed, graceful stop begins
    assert_eq!(h.status(id).await, OptimizationStatus::Stopping);
    assert_eq!(h.log.starting_with("runners.stop").len(), 2);
    assert!(h.compute.identity(&b.agent(Codex)).unwrap().revoked);
    let statuses: Vec<_> = h
        .ledger
        .list_agents(&oid(id))
        .await
        .unwrap()
        .into_iter()
        .map(|a| a.status)
        .collect();
    assert_eq!(statuses, [AgentStatus::Stopped, AgentStatus::Stopped]);
    assert_eq!(
        candidates(&h, id).await[0].candidate.verdict,
        CandidateVerdict::Pending
    ); // confirm still due

    h.until(
        async || h.status(id).await == OptimizationStatus::Stopped,
        60,
    )
    .await;
    assert_eq!(h.verdicts(id).await, v(&[("sha-late", "verified")]));
    assert_eq!(
        h.engine.stop_reason(id).await.unwrap().as_deref(),
        Some("time limit reached")
    );
    assert!(h.compute.live(None).is_empty());
    assert!(
        h.ledger
            .get_optimization(&oid(id))
            .await
            .unwrap()
            .unwrap()
            .ended_at
            .is_some()
    );
}

#[tokio::test]
async fn picks_up_a_candidate_pushed_right_before_a_graceful_stop_final_poll() {
    let h = Harness::new(Default::default()).await;
    let b = h
        .begin(BeginOptions {
            time_limit_seconds: Some(5 * 60),
            ..Default::default()
        })
        .await
        .unwrap();
    h.cycle(4).await;
    h.candidate(CC, "sha-last", win(85.0));
    h.until(
        async || h.status(&b.id).await == OptimizationStatus::Stopped,
        60,
    )
    .await;
    assert_eq!(h.verdicts(&b.id).await, v(&[("sha-last", "verified")]));
}

mod graceful_stop_drains_every_candidate {
    use super::*;

    #[tokio::test]
    async fn all_agents_done_in_the_same_tick_as_their_last_push() {
        let h = Harness::new(Default::default()).await;
        let b = begun(&h).await;
        h.candidate(CX, "sha-last", win(85.0));
        h.runners.emit(&b.agent(ClaudeCode), vec![done_event()]);
        h.runners.emit(&b.agent(Codex), vec![done_event()]);
        h.until(
            async || h.status(&b.id).await == OptimizationStatus::Stopped,
            60,
        )
        .await;
        assert_eq!(
            h.engine.stop_reason(&b.id).await.unwrap().as_deref(),
            Some("all agents finished")
        );
        assert_eq!(h.verdicts(&b.id).await, v(&[("sha-last", "verified")]));
        let phases: Vec<_> = candidates(&h, &b.id).await[0]
            .results
            .iter()
            .map(|r| r.phase.to_string())
            .collect();
        assert_eq!(phases, ["bench", "confirm"]);
        assert_eq!(
            h.ledger.current_best(&oid(&b.id)).await.unwrap(),
            Some(85.0)
        );
    }

    #[tokio::test]
    async fn a_runner_that_pushes_its_last_commit_while_it_is_being_stopped_is_still_collected() {
        let h = Harness::new(Default::default()).await;
        let b = h
            .begin(BeginOptions {
                time_limit_seconds: Some(5 * 60),
                ..Default::default()
            })
            .await
            .unwrap();
        let (git, bench, runners, codex) = (
            h.git.clone(),
            h.bench.clone(),
            h.runners.clone(),
            b.agent(Codex),
        );
        *h.runners.stop_hook.lock().unwrap() = Some(Arc::new(move |handle| {
            if handle.agent_id.as_str() == codex {
                git.push(CX, "sha-in-flight", &["src/search.ts"]);
                bench.set(
                    "sha-in-flight",
                    "bench",
                    ScriptedBench::samples(&[70.0; 3]).with_guardrails(vec![passed("pytest -q")]),
                );
                bench.set(
                    "sha-in-flight",
                    "confirm",
                    ScriptedBench::samples(&[70.0; 3]),
                );
                runners.emit(handle.agent_id.as_str(), vec![done_event()]);
            }
        }));
        h.until(
            async || h.status(&b.id).await == OptimizationStatus::Stopped,
            60,
        )
        .await;
        assert_eq!(h.verdicts(&b.id).await, v(&[("sha-in-flight", "verified")]));
    }

    #[tokio::test]
    async fn reports_done_only_after_the_push_becomes_visible_the_drain_keeps_polling_git() {
        let h = Harness::new(Default::default()).await;
        let b = h
            .begin(BeginOptions {
                time_limit_seconds: Some(5 * 60),
                ..Default::default()
            })
            .await
            .unwrap();
        // The branch listing lags: the first two polls after the stop see nothing, then the commit appears.
        h.candidate(CC, "sha-lagging", win(60.0));
        let (ledger, hidden) = (h.ledger.clone(), Arc::new(AtomicUsize::new(0)));
        *h.git.list_hook.lock().unwrap() = Some(Arc::new(move |_branch, _since| {
            let (ledger, hidden) = (ledger.clone(), hidden.clone());
            Box::pin(async move {
                let st = ledger
                    .get_optimization(&oid("opt-test"))
                    .await
                    .unwrap()
                    .unwrap()
                    .status;
                if st == OptimizationStatus::Stopping && hidden.load(Ordering::SeqCst) < 2 {
                    hidden.fetch_add(1, Ordering::SeqCst);
                    return Some(Ok(vec![]));
                }
                None
            })
        }));
        // Make the commit invisible to the normal running polls too, so only the drain can find it.
        let running = std::mem::take(h.git.commits.lock().unwrap().get_mut(CC).unwrap());
        h.runners.emit(&b.agent(ClaudeCode), vec![done_event()]);
        h.runners.emit(&b.agent(Codex), vec![done_event()]);
        h.cycle(1).await;
        h.git.commits.lock().unwrap().insert(CC.into(), running);
        h.until(
            async || h.status(&b.id).await == OptimizationStatus::Stopped,
            60,
        )
        .await;
        assert_eq!(h.verdicts(&b.id).await, v(&[("sha-lagging", "verified")]));
    }

    #[tokio::test]
    async fn survives_a_transient_git_failure_during_the_final_poll() {
        let h = Harness::new(Default::default()).await;
        let b = begun(&h).await;
        h.candidate(CX, "sha-flaky", win(88.0));
        let failures = Arc::new(AtomicUsize::new(0));
        *h.git.list_hook.lock().unwrap() = Some(Arc::new(move |_b, _s| {
            let failures = failures.clone();
            Box::pin(async move {
                if failures.fetch_add(1, Ordering::SeqCst) < 3 {
                    return Some(Err(PortError::failed("git fetch lock contention")));
                }
                None
            })
        }));
        h.runners.emit(&b.agent(ClaudeCode), vec![done_event()]);
        h.runners.emit(&b.agent(Codex), vec![done_event()]);
        h.until(
            async || h.status(&b.id).await == OptimizationStatus::Stopped,
            60,
        )
        .await;
        assert_eq!(h.verdicts(&b.id).await, v(&[("sha-flaky", "verified")]));
    }

    #[tokio::test]
    async fn an_agent_that_finishes_while_the_others_keep_running_still_gets_its_last_push_submitted()
     {
        let h = Harness::new(Default::default()).await;
        let b = begun(&h).await;
        h.candidate(CC, "sha-final", win(75.0));
        h.runners.emit(&b.agent(ClaudeCode), vec![done_event()]);
        h.cycle(1).await;
        assert_eq!(
            h.ledger
                .get_agent(&b.agent(ClaudeCode).as_str().into())
                .await
                .unwrap()
                .unwrap()
                .status,
            AgentStatus::Done
        );
        assert_eq!(h.status(&b.id).await, OptimizationStatus::Running);
        assert_eq!(
            h.verdicts(&b.id).await.keys().collect::<Vec<_>>(),
            ["sha-final"]
        );
        h.cycle(4).await;
        assert_eq!(h.verdicts(&b.id).await, v(&[("sha-final", "verified")]));
    }

    #[tokio::test]
    async fn evaluates_a_backlog_queued_from_several_agents_before_terminating() {
        let h = Harness::new(Default::default()).await;
        let b = h
            .begin(BeginOptions {
                time_limit_seconds: Some(5 * 60),
                ..Default::default()
            })
            .await
            .unwrap();
        h.candidate(CC, "sha-b1", win(90.0));
        h.candidate(CX, "sha-b2", win(80.0));
        h.candidate(CC, "sha-b3", win(70.0));
        h.runners.emit(&b.agent(ClaudeCode), vec![done_event()]);
        h.runners.emit(&b.agent(Codex), vec![done_event()]);
        h.until(
            async || h.status(&b.id).await == OptimizationStatus::Stopped,
            60,
        )
        .await;
        let verdicts = h.verdicts(&b.id).await;
        assert!(!verdicts.values().any(|v| v == "pending"));
        assert_eq!(
            verdicts.keys().cloned().collect::<Vec<_>>(),
            ["sha-b1", "sha-b2", "sha-b3"]
        );
        assert_eq!(
            h.ledger.current_best(&oid(&b.id)).await.unwrap(),
            Some(70.0)
        );
        assert!(h.compute.live(None).is_empty());
    }

    #[tokio::test]
    async fn the_drain_is_bounded_by_drain_timeout_ms() {
        let h = Harness::new(HarnessOptions {
            settings: ratchet_engine::EngineSettings {
                drain_timeout_ms: Some(3 * TICK_MS),
                ..Default::default()
            },
            ..Default::default()
        })
        .await;
        let b = h
            .begin(BeginOptions {
                time_limit_seconds: Some(5 * 60),
                ..Default::default()
            })
            .await
            .unwrap();
        h.bench.close_gate();
        h.candidate(CX, "sha-slow", win(50.0));
        h.runners.emit(&b.agent(ClaudeCode), vec![done_event()]);
        h.runners.emit(&b.agent(Codex), vec![done_event()]);
        for _ in 0..12 {
            if h.status(&b.id).await == OptimizationStatus::Stopped {
                break;
            }
            h.clock.advance(TICK_MS);
            h.engine.tick().await.unwrap();
        }
        h.bench.open_gate();
        assert_eq!(h.status(&b.id).await, OptimizationStatus::Stopped);
        assert!(
            h.engine
                .stop_reason(&b.id)
                .await
                .unwrap()
                .unwrap()
                .contains("drain timeout")
        );
    }
}

mod identical_shas_from_different_agents {
    use super::*;

    #[tokio::test]
    async fn keys_candidates_per_agent_and_sha() {
        let h = Harness::new(Default::default()).await;
        let b = begun(&h).await;
        h.git.push(CC, "sha-same", &["src/search.ts"]);
        h.git.push(CX, "sha-same", &["src/search.ts"]);
        h.bench.set(
            "sha-same",
            "bench",
            ScriptedBench::samples(&[80.0; 3]).with_guardrails(vec![passed("pytest -q")]),
        );
        h.bench
            .set("sha-same", "confirm", ScriptedBench::samples(&[80.0; 3]));
        h.until(async || all_decided(&h, &b.id, 2).await, 60).await;

        let rows = candidates(&h, &b.id).await;
        let ids: std::collections::HashSet<_> =
            rows.iter().map(|c| c.candidate.id.to_string()).collect();
        assert_eq!(ids.len(), 2);
        let (claude, codex) = (b.agent(ClaudeCode), b.agent(Codex));
        let first = rows
            .iter()
            .find(|c| c.candidate.agent_id.as_str() == claude)
            .unwrap();
        let second = rows
            .iter()
            .find(|c| c.candidate.agent_id.as_str() == codex)
            .unwrap();
        assert_eq!(first.candidate.verdict, CandidateVerdict::Verified);
        assert_eq!(second.candidate.verdict, CandidateVerdict::Rejected);
        assert!(
            second
                .candidate
                .reasons
                .join(" ")
                .contains("not better than current best 80")
        );
        assert_eq!(
            second
                .results
                .iter()
                .map(|r| r.phase.to_string())
                .collect::<Vec<_>>(),
            ["bench"]
        );
        let hist = h.ledger.ratchet_history(&oid(&b.id)).await.unwrap();
        assert_eq!(
            hist.iter().map(|m| m.sha.as_str()).collect::<Vec<_>>(),
            ["sha-same"]
        );
        assert_eq!(
            h.ledger.current_best(&oid(&b.id)).await.unwrap(),
            Some(80.0)
        );
        // each agent hears about its own candidate only
        assert_eq!(
            sent_verdicts(&h, &claude),
            [("sha-same".to_owned(), CandidateVerdict::Verified)]
        );
        assert_eq!(
            sent_verdicts(&h, &codex),
            [("sha-same".to_owned(), CandidateVerdict::Rejected)]
        );
        // re-listing does not duplicate either row
        h.cycle(3).await;
        assert_eq!(candidates(&h, &b.id).await.len(), 2);
    }

    #[tokio::test]
    async fn a_later_commit_on_one_branch_is_found_even_when_another_agent_already_submitted_the_same_sha()
     {
        let h = Harness::new(Default::default()).await;
        let b = begun(&h).await;
        h.git.push(CC, "sha-same", &["src/search.ts"]);
        h.bench
            .set("sha-same", "bench", ScriptedBench::samples(&[90.0; 3]));
        h.bench
            .set("sha-same", "confirm", ScriptedBench::samples(&[90.0; 3]));
        h.cycle(6).await;
        h.git.push(CX, "sha-same", &["src/search.ts"]);
        h.candidate(CX, "sha-next", win(70.0));
        h.until(async || all_decided(&h, &b.id, 3).await, 60).await;
        assert_eq!(h.verdicts(&b.id).await["sha-next"], "verified");
    }
}

#[tokio::test]
async fn stops_after_plateau_evals_consecutive_evaluations_without_a_verified_win_a_win_resets_the_count()
 {
    let h = Harness::new(Default::default()).await;
    let b = h
        .begin(BeginOptions {
            agents: Some(vec![ClaudeCode]),
            plateau_evals: Some(3),
            ..Default::default()
        })
        .await
        .unwrap();
    h.candidate(CC, "sha-r1", Cand::new().bench(&[100.0; 3]));
    h.candidate(CC, "sha-r2", Cand::new().bench(&[101.0; 3]));
    h.candidate(CC, "sha-v1", win(80.0));
    h.candidate(CC, "sha-r3", Cand::new().bench(&[80.0; 3]));
    h.candidate(CC, "sha-r4", Cand::new().bench(&[81.0; 3]));
    h.until(async || all_decided(&h, &b.id, 5).await, 60).await;
    assert_eq!(h.status(&b.id).await, OptimizationStatus::Running); // streak is 2, not 3
    h.candidate(CC, "sha-r5", Cand::new().bench(&[82.0; 3]));
    h.until(
        async || h.status(&b.id).await == OptimizationStatus::Stopped,
        60,
    )
    .await;
    assert!(
        h.engine
            .stop_reason(&b.id)
            .await
            .unwrap()
            .unwrap()
            .contains("plateau: 3 consecutive")
    );
    assert_eq!(h.verdicts(&b.id).await["sha-r5"], "rejected"); // the third eval was decided before stopping
    assert!(h.compute.live(None).is_empty());
}

#[tokio::test]
async fn counts_blocked_candidates_toward_the_plateau() {
    let h = Harness::new(Default::default()).await;
    let b = h
        .begin(BeginOptions {
            agents: Some(vec![Codex]),
            plateau_evals: Some(2),
            ..Default::default()
        })
        .await
        .unwrap();
    h.candidate(CX, "sha-b1", Cand::new().files(&["ratchet.yaml"]));
    h.candidate(CX, "sha-b2", Cand::new().files(&["bench/x"]));
    h.until(
        async || h.status(&b.id).await == OptimizationStatus::Stopped,
        60,
    )
    .await;
    assert!(
        h.engine
            .stop_reason(&b.id)
            .await
            .unwrap()
            .unwrap()
            .contains("plateau")
    );
}

#[tokio::test]
async fn stops_when_every_agent_reports_done() {
    let h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    h.runners.emit(&b.agent(ClaudeCode), vec![done_event()]);
    h.runners.emit(
        &b.agent(Codex),
        vec![status_event(serde_json::json!({ "state": "failed" }))],
    );
    h.until(
        async || h.status(&b.id).await == OptimizationStatus::Stopped,
        60,
    )
    .await;
    assert_eq!(
        h.engine.stop_reason(&b.id).await.unwrap().as_deref(),
        Some("all agents finished")
    );
    assert_eq!(
        h.ledger
            .get_agent(&b.agent(ClaudeCode).as_str().into())
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Done
    );
    assert_eq!(
        h.ledger
            .get_agent(&b.agent(Codex).as_str().into())
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Failed
    );
}

// ---------------------------------------------------------------------------------------- runner finish events

#[tokio::test]
async fn treats_the_runner_loops_closing_status_as_done_or_failed_and_stops_once_every_agent_is_finished()
 {
    let h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    let fin = |reason: &str| {
        ev(AgentEventType::Status)
            .with_text(format!("runner finished: {reason}"))
            .with_data(serde_json::json!({ "reason": reason, "turns": 3, "candidates": 1, "spendUsd": 0.1 }))
    };
    h.runners.emit(&b.agent(ClaudeCode), vec![fin("idle")]);
    h.runners
        .emit(&b.agent(Codex), vec![fin("agent-unavailable")]);
    h.until(
        async || h.status(&b.id).await == OptimizationStatus::Stopped,
        60,
    )
    .await;
    assert_eq!(
        h.engine.stop_reason(&b.id).await.unwrap().as_deref(),
        Some("all agents finished")
    );
    assert_eq!(
        h.ledger
            .get_agent(&b.agent(ClaudeCode).as_str().into())
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Done
    );
    assert_eq!(
        h.ledger
            .get_agent(&b.agent(Codex).as_str().into())
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Failed
    );
}

#[tokio::test]
async fn a_runner_that_finished_because_the_engine_stopped_it_is_not_treated_as_done_early() {
    let h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    h.runners.emit(
        &b.agent(ClaudeCode),
        vec![
            ev(AgentEventType::Status)
                .with_text("runner finished: stopped")
                .with_data(serde_json::json!({ "reason": "stopped", "turns": 1, "candidates": 0 })),
        ],
    );
    h.cycle(2).await;
    assert_eq!(
        h.ledger
            .get_agent(&b.agent(ClaudeCode).as_str().into())
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Running
    );
    assert_eq!(h.status(&b.id).await, OptimizationStatus::Running);
}

// ---------------------------------------------------------------------------------------- kill switch

#[tokio::test]
async fn stop_revokes_first_then_stops_runners_then_terminates_and_aborts_the_in_flight_bench() {
    let h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    h.compute.launch_experiment(&b.id, &b.agent(Codex));
    h.candidate(CX, "sha-1", win(80.0));
    h.bench.close_gate();
    h.clock.advance(TICK_MS);
    h.engine.tick().await.unwrap();
    assert!(h.engine.busy());
    let mark = h.log.len();
    h.engine
        .stop(&b.id, Some("user pressed stop"))
        .await
        .unwrap();
    h.engine.settle().await; // the aborted job unwinds

    let after: Vec<_> = h
        .log
        .since(mark)
        .into_iter()
        .map(|l| l.split(':').next().unwrap().to_owned())
        .collect();
    assert_eq!(
        after,
        [
            "compute.revoke",
            "compute.revoke",
            "runners.stop",
            "runners.stop",
            "compute.terminate"
        ]
    );
    assert!(h.compute.live(None).is_empty());
    assert_eq!(h.status(&b.id).await, OptimizationStatus::Stopped);
    assert_eq!(
        h.engine.stop_reason(&b.id).await.unwrap().as_deref(),
        Some("user pressed stop")
    );
    assert_eq!(
        candidates(&h, &b.id).await[0].candidate.verdict,
        CandidateVerdict::Pending
    ); // left undecided, not mis-scored
    // the aborted bench never produces a verdict afterwards
    h.bench.open_gate();
    h.cycle(3).await;
    assert_eq!(
        candidates(&h, &b.id).await[0].candidate.verdict,
        CandidateVerdict::Pending
    );
}

#[tokio::test]
async fn retries_a_failed_revocation_on_later_ticks_and_keeps_terminating_late_instances() {
    let h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    let codex = b.agent(Codex);
    h.compute.fail_revoke.lock().unwrap().insert(codex.clone());
    h.engine.stop(&b.id, None).await.unwrap();
    assert!(!h.compute.identity(&codex).unwrap().revoked);
    assert!(h.compute.identity(&b.agent(ClaudeCode)).unwrap().revoked);
    h.compute.fail_revoke.lock().unwrap().clear();
    // an experiment that was mid-launch when the switch was thrown shows up afterwards
    h.compute.launch_experiment(&b.id, &codex);
    h.cycle(1).await;
    assert!(h.compute.identity(&codex).unwrap().revoked);
    assert!(h.compute.live(None).is_empty());
}

#[tokio::test]
async fn stop_is_idempotent_and_a_stopped_optimization_stays_stopped() {
    let h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    h.engine.stop(&b.id, None).await.unwrap();
    let n = h.log.starting_with("runners.stop").len();
    h.engine.stop(&b.id, None).await.unwrap();
    assert_eq!(h.log.starting_with("runners.stop").len(), n);
    assert_eq!(h.status(&b.id).await, OptimizationStatus::Stopped);
}

// ---------------------------------------------------------------------------------------- reaper

#[tokio::test]
async fn terminates_experiments_past_their_max_lifetime_and_instances_of_a_stopped_or_finished_agent()
 {
    let h = Harness::new(HarnessOptions {
        hourly: Some(0.01),
        ..Default::default()
    })
    .await;
    let b = begun(&h).await;
    let old = h.compute.launch_experiment(&b.id, &b.agent(Codex));
    h.cycle(1).await;
    assert_eq!(
        h.compute.instance(&old.id).unwrap().state,
        InstanceState::Running
    );
    h.clock.advance(2 * 3_600_000 + 1);
    h.engine.tick().await.unwrap();
    assert_eq!(
        h.compute.instance(&old.id).unwrap().state,
        InstanceState::Terminated
    );

    // an agent that reports done: its experiments go with it
    let exp2 = h.compute.launch_experiment(&b.id, &b.agent(ClaudeCode));
    h.runners.emit(&b.agent(ClaudeCode), vec![done_event()]);
    h.cycle(1).await;
    h.cycle(1).await;
    assert_eq!(
        h.compute.instance(&exp2.id).unwrap().state,
        InstanceState::Terminated
    );
    let codex_runner = h
        .ledger
        .get_agent(&b.agent(Codex).as_str().into())
        .await
        .unwrap()
        .unwrap()
        .runner_instance_id
        .unwrap();
    assert_eq!(
        h.compute.instance(&codex_runner).unwrap().state,
        InstanceState::Running
    );
}

#[tokio::test]
async fn terminates_agent_runners_that_outlive_the_time_limit_plus_grace() {
    let h = Harness::new(HarnessOptions {
        hourly: Some(0.01),
        settings: ratchet_engine::EngineSettings {
            reaper: ratchet_engine::ReaperSettings {
                runner_grace_ms: Some(0),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    })
    .await;
    let b = h
        .begin(BeginOptions {
            time_limit_seconds: Some(3600),
            ..Default::default()
        })
        .await
        .unwrap();
    h.clock.advance(3600 * 1000 + 1);
    h.engine.tick().await.unwrap();
    assert_eq!(h.status(&b.id).await, OptimizationStatus::Stopping); // graceful stop first
    h.until(
        async || h.status(&b.id).await == OptimizationStatus::Stopped,
        60,
    )
    .await;
    assert!(h.compute.live(None).is_empty());
}

// ---------------------------------------------------------------------------------------- agent control

#[tokio::test]
async fn routes_pause_resume_message_to_the_right_runner_and_tracks_status() {
    let h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    let (claude, codex) = (b.agent(ClaudeCode), b.agent(Codex));

    h.engine.pause_agent(&claude).await.unwrap();
    assert_eq!(
        h.ledger
            .get_agent(&claude.as_str().into())
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Paused
    );
    assert_eq!(h.runners.sent_to(&claude), [RunnerMessage::Pause]);
    assert!(h.runners.sent_to(&codex).is_empty());
    h.engine.pause_agent(&claude).await.unwrap(); // no-op
    assert_eq!(h.runners.sent_to(&claude).len(), 1);

    h.engine
        .message_agent(&codex, "try caching the tokenizer")
        .await
        .unwrap();
    assert_eq!(
        h.runners.sent_to(&codex),
        [RunnerMessage::User {
            text: "try caching the tokenizer".into()
        }]
    );
    let events = h
        .ledger
        .agent_events(&codex.as_str().into(), 0, 1000)
        .await
        .unwrap();
    assert!(events.iter().any(|e| e.event_type == "message"));

    // a paused agent still has its events and candidates collected
    h.candidate(CC, "sha-p", win(80.0));
    h.runners.emit(&claude, vec![msg_event("still thinking")]);
    h.cycle(4).await;
    assert_eq!(h.verdicts(&b.id).await["sha-p"], "verified");
    let events = h
        .ledger
        .agent_events(&claude.as_str().into(), 0, 1000)
        .await
        .unwrap();
    assert!(events.iter().any(|e| {
        e.event_type == "message"
            && e.payload
                .as_ref()
                .is_some_and(|p| p.to_string().contains("still thinking"))
    }));

    h.engine.resume_agent(&claude).await.unwrap();
    assert_eq!(
        h.ledger
            .get_agent(&claude.as_str().into())
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Running
    );
    assert_eq!(
        h.runners.sent_to(&claude).last(),
        Some(&RunnerMessage::Resume)
    );

    assert_eq!(
        code(h.engine.message_agent(&codex, "  ").await),
        C::InvalidInput
    );
    h.engine.stop(&b.id, None).await.unwrap();
    assert_eq!(code(h.engine.pause_agent(&codex).await), C::InvalidState);
    assert_eq!(
        code(h.engine.message_agent(&codex, "hi").await),
        C::InvalidState
    );
}

#[tokio::test]
async fn survives_a_runner_that_cannot_be_reached() {
    let h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    let codex = b.agent(Codex);
    h.runners.fail_poll.lock().unwrap().insert(codex.clone());
    h.runners.fail_send.lock().unwrap().insert(codex.clone());
    h.runners
        .emit(&b.agent(ClaudeCode), vec![msg_event("hello")]);
    h.candidate(CC, "sha-1", win(80.0));
    h.until(
        async || h.verdicts(&b.id).await.get("sha-1").map(String::as_str) == Some("verified"),
        60,
    )
    .await; // announcement to codex fails silently
    assert!(
        h.engine
            .errors()
            .iter()
            .any(|e| e.context.starts_with("poll"))
    );
    let events = h
        .ledger
        .agent_events(&codex.as_str().into(), 0, 1000)
        .await
        .unwrap();
    assert!(events.iter().any(|e| e.event_type == "error"));
    assert_eq!(h.status(&b.id).await, OptimizationStatus::Running);
}

// ---------------------------------------------------------------------------------------- restart and resume

#[tokio::test]
async fn re_polls_runners_from_their_stored_cursors_no_lost_and_no_replayed_events() {
    let mut h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    let claude = b.agent(ClaudeCode);
    h.runners
        .emit(&claude, vec![msg_event("one"), msg_event("two")]);
    h.cycle(1).await;
    let messages = |h: &Harness| {
        let (l, c) = (h.ledger.clone(), claude.clone());
        async move {
            l.agent_events(&c.as_str().into(), 0, 1000)
                .await
                .unwrap()
                .into_iter()
                .filter(|e| e.event_type == "message")
                .map(|e| e.payload.unwrap()["text"].as_str().unwrap().to_owned())
                .collect::<Vec<_>>()
        }
    };
    assert_eq!(messages(&h).await.len(), 2);

    let e2 = h.restart(false).await;
    e2.ready().await;
    h.runners.emit(&claude, vec![msg_event("three")]);
    h.runners.poll_calls.lock().unwrap().clear();
    h.cycle(2).await;
    let first = h
        .runners
        .poll_calls
        .lock()
        .unwrap()
        .iter()
        .find(|(a, _)| *a == claude)
        .cloned()
        .unwrap();
    assert_eq!(first.1.as_deref(), Some("2"));
    assert_eq!(messages(&h).await, ["one", "two", "three"]);
    assert_eq!(h.status(&b.id).await, OptimizationStatus::Running);
}

#[tokio::test]
async fn finishes_a_bench_that_was_in_flight_when_the_process_died() {
    let mut h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    h.candidate(CX, "sha-1", win(80.0));
    h.clock.advance(TICK_MS);
    h.engine.tick().await.unwrap(); // candidate submitted, bench job started...
    h.engine.settle().await; // ...and finished, but the outcome was never collected
    assert!(candidates(&h, &b.id).await[0].results.is_empty());
    let bench_runs = h.bench.job_count();

    h.restart(false).await;
    h.until(
        async || h.verdicts(&b.id).await.get("sha-1").map(String::as_str) == Some("verified"),
        60,
    )
    .await;
    assert_eq!(h.bench.job_count(), bench_runs + 2); // bench re-run once + confirm
    assert_eq!(
        h.ledger.ratchet_history(&oid(&b.id)).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn continues_with_the_confirm_run_when_the_crash_came_between_bench_and_confirm() {
    let mut h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    h.candidate(CX, "sha-1", win(80.0));
    h.cycle(2).await; // bench recorded, confirm job done but uncollected
    let c = &candidates(&h, &b.id).await[0];
    assert_eq!(
        c.results
            .iter()
            .map(|r| r.phase.to_string())
            .collect::<Vec<_>>(),
        ["bench"]
    );
    let jobs = h.bench.job_count();
    h.restart(false).await;
    h.until(
        async || h.verdicts(&b.id).await.get("sha-1").map(String::as_str) == Some("verified"),
        60,
    )
    .await;
    let after: Vec<_> = h
        .bench
        .jobs()
        .into_iter()
        .skip(jobs)
        .map(|j| phase_str(j.phase))
        .collect();
    assert_eq!(after, ["confirm"]); // bench is not repeated
}

#[tokio::test]
async fn settles_a_candidate_whose_results_were_all_written_but_whose_verdict_was_not() {
    // the crash window: both results are in the ledger, the verdict and ratchet move were lost
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(HarnessOptions {
        ledger_path: Some(dir.path().join("l.db")),
        ..Default::default()
    })
    .await;
    let b = begun(&h).await;
    h.candidate(CX, "sha-2", Cand::new());
    h.bench
        .set("sha-2", "bench", ScriptedBench::samples(&[80.0; 3]));
    h.clock.advance(TICK_MS);
    h.engine.tick().await.unwrap();
    h.engine.settle().await;
    let cand = candidates(&h, &b.id).await.remove(0);
    let now = h.clock.now();
    for (phase, s) in [
        (ratchet_core::enums::BenchPhase::Bench, 80.0),
        (ratchet_core::enums::BenchPhase::Confirm, 79.0),
    ] {
        h.ledger
            .record_bench_result(ratchet_ledger::RecordBenchResultInput {
                candidate_id: cand.candidate.id.clone(),
                phase,
                samples: vec![s; 3],
                stats: ratchet_core::rows::BenchStats::new(s),
                guardrails: vec![],
                started_at: now,
                finished_at: now,
            })
            .await
            .unwrap();
    }
    h.restart(true).await;
    let jobs = h.bench.job_count();
    h.cycle(2).await;
    assert_eq!(h.verdicts(&b.id).await["sha-2"], "verified");
    assert_eq!(h.bench.job_count(), jobs); // nothing re-run
    assert_eq!(
        h.ledger.current_best(&oid(&b.id)).await.unwrap(),
        Some(80.0)
    );
}

#[tokio::test]
async fn resumes_from_a_sqlite_file_after_the_process_dies_mid_run() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(HarnessOptions {
        ledger_path: Some(dir.path().join("ledger.db")),
        ..Default::default()
    })
    .await;
    let b = begun(&h).await;
    h.candidate(CC, "sha-c1", win(90.0));
    h.candidate(CC, "sha-c2", win(70.0));
    h.runners.emit(&b.agent(Codex), vec![msg_event("a")]);
    h.until(
        async || h.verdicts(&b.id).await.get("sha-c1").map(String::as_str) == Some("verified"),
        60,
    )
    .await;

    let e2 = h.restart(true).await;
    assert_eq!(e2.ready().await.attached, [b.id.clone()]);
    h.until(
        async || h.verdicts(&b.id).await.get("sha-c2").map(String::as_str) == Some("verified"),
        60,
    )
    .await;
    let hist = h.ledger.ratchet_history(&oid(&b.id)).await.unwrap();
    assert_eq!(
        hist.iter().map(|m| m.to_value).collect::<Vec<_>>(),
        [90.0, 70.0]
    );
    let codex_msgs = h
        .ledger
        .agent_events(&b.agent(Codex).as_str().into(), 0, 1000)
        .await
        .unwrap();
    assert_eq!(
        codex_msgs
            .iter()
            .filter(|e| e.event_type == "message")
            .count(),
        1
    );

    // the deadline and plateau settings survived too
    h.clock.advance(9 * 3600 * 1000);
    h.until(
        async || h.status(&b.id).await == OptimizationStatus::Stopped,
        60,
    )
    .await;
    assert_eq!(
        h.engine.stop_reason(&b.id).await.unwrap().as_deref(),
        Some("time limit reached")
    );
    assert_eq!(
        h.ledger
            .get_baseline(&oid(&b.id))
            .await
            .unwrap()
            .unwrap()
            .value,
        100.0
    );
}

#[tokio::test]
async fn finishes_launching_half_launched_agents_after_a_hard_crash_without_repeating_completed_steps()
 {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(HarnessOptions {
        ledger_path: Some(dir.path().join("ledger.db")),
        ..Default::default()
    })
    .await;
    h.bench
        .set("main-sha", "baseline", ScriptedBench::samples(&[100.0; 3]));
    h.bench
        .set("main-sha", "confirm", ScriptedBench::samples(&[100.0; 3]));
    let id = h
        .engine
        .create_optimization(CreateOptimizationInput {
            agents: vec![ClaudeCode, Codex],
            budget_usd: 100.0,
            time_limit_seconds: 3600,
            plateau_evals: 5,
            ..small_input()
        })
        .await
        .unwrap();
    // the process dies while the second runner is being launched
    h.compute.hang_launch_at.store(2, Ordering::SeqCst);
    let e = h.engine.clone();
    let id2 = id.clone();
    let _doomed = tokio::spawn(async move { e.start(&id2).await });
    for _ in 0..200 {
        if h.log.starting_with("compute.launchRunner").len() >= 2 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    assert_eq!(h.status(&id).await, OptimizationStatus::Baselining);
    assert!(h.ledger.get_baseline(&oid(&id)).await.unwrap().is_some());

    h.compute.hang_launch_at.store(0, Ordering::SeqCst);
    h.restart(true).await;
    h.engine.ready().await;
    for _ in 0..300 {
        if h.status(&id).await == OptimizationStatus::Running {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert_eq!(h.status(&id).await, OptimizationStatus::Running);
    let statuses: Vec<_> = h
        .ledger
        .list_agents(&oid(&id))
        .await
        .unwrap()
        .into_iter()
        .map(|a| a.status)
        .collect();
    assert_eq!(statuses, [AgentStatus::Running, AgentStatus::Running]);
    assert_eq!(h.log.starting_with("compute.createAgentIdentity").len(), 2); // not repeated
    assert_eq!(h.log.starting_with("runners.start").len(), 2);
    assert_eq!(h.log.starting_with("git.createBranch").len(), 2);
    assert_eq!(
        h.bench
            .jobs()
            .iter()
            .filter(|j| phase_str(j.phase) == "baseline")
            .count(),
        1
    ); // baseline not repeated
}

#[tokio::test]
async fn completes_a_kill_switch_that_was_interrupted() {
    let dir = tempfile::tempdir().unwrap();
    let mut h = Harness::new(HarnessOptions {
        ledger_path: Some(dir.path().join("ledger.db")),
        ..Default::default()
    })
    .await;
    let b = begun(&h).await;
    // a crash right after the stop marker was written
    h.ledger
        .set_optimization_status(&oid(&b.id), OptimizationStatus::Stopping, None)
        .await
        .unwrap();
    h.ledger
        .kv_set(&format!("opt:{}:stop_mode", b.id), "kill")
        .await
        .unwrap();
    h.restart(true).await;
    h.engine.ready().await;
    h.until(
        async || h.status(&b.id).await == OptimizationStatus::Stopped,
        500, // the resumed kill switch runs on real I/O; five cycles failed once on a loaded machine
    )
    .await;
    assert!(h.compute.live(None).is_empty());
    assert!(
        h.compute
            .identities
            .lock()
            .unwrap()
            .values()
            .all(|i| i.revoked)
    );
}

// ---------------------------------------------------------------------------------------- teardown

#[tokio::test]
async fn teardown_deletes_secrets_and_identities_terminates_leftovers_verifies_zero_instances_and_completes()
 {
    let h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    h.compute.launch_experiment(&b.id, &b.agent(Codex));
    h.engine.teardown(&b.id).await.unwrap(); // stops a running optimization first
    assert_eq!(h.status(&b.id).await, OptimizationStatus::Completed);
    assert!(h.compute.live(None).is_empty());
    assert_eq!(h.compute.secret_count(), 0);
    assert!(
        h.compute
            .identities
            .lock()
            .unwrap()
            .values()
            .all(|i| i.revoked && i.deleted)
    );
    let report = h.engine.teardown_report(&b.id).await.unwrap().unwrap();
    assert!(report.orphans.is_empty() && report.errors.is_empty());
    assert_eq!(
        report.deleted_secrets_for,
        [b.agent(ClaudeCode), b.agent(Codex)]
    );
    assert!(
        h.artifacts
            .text("engine/opt-test/teardown.json")
            .unwrap()
            .contains("\"orphans\": []")
    );
    // idempotent
    h.engine.teardown(&b.id).await.unwrap();
}

#[tokio::test]
async fn teardown_refuses_to_report_success_while_an_instance_survives_termination() {
    let h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    let stuck = h.compute.launch_experiment(&b.id, &b.agent(Codex));
    h.compute.stubborn.lock().unwrap().insert(stuck.id.clone());
    assert_eq!(code(h.engine.teardown(&b.id).await), C::Orphans);
    assert_eq!(h.status(&b.id).await, OptimizationStatus::Stopped);
    let report = h.engine.teardown_report(&b.id).await.unwrap().unwrap();
    assert_eq!(
        report
            .orphans
            .iter()
            .map(|o| o.id.clone())
            .collect::<Vec<_>>(),
        [stuck.id.clone()]
    );
    h.compute.stubborn.lock().unwrap().clear();
    h.engine.teardown(&b.id).await.unwrap(); // retry succeeds
    assert_eq!(h.status(&b.id).await, OptimizationStatus::Completed);
    assert!(h.compute.live(None).is_empty());
}

#[tokio::test]
async fn teardown_reports_a_failed_secret_deletion_instead_of_completing() {
    let h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    h.compute.fail_delete_secrets.store(true, Ordering::SeqCst);
    assert_eq!(code(h.engine.teardown(&b.id).await), C::TeardownIncomplete);
    assert_eq!(h.status(&b.id).await, OptimizationStatus::Stopped);
}

#[tokio::test]
async fn can_tear_down_a_draft_that_never_started() {
    let h = Harness::new(Default::default()).await;
    let id = h.engine.create_optimization(small_input()).await.unwrap();
    h.engine.teardown(&id).await.unwrap();
    assert_eq!(h.status(&id).await, OptimizationStatus::Completed);
}

// ---------------------------------------------------------------------------------------- run loop and events

struct CountingClock {
    inner: Arc<ratchet_core::ports::mock::ManualClock>,
    sleeps: Arc<AtomicUsize>,
    cancel: ratchet_core::ports::CancellationToken,
}

#[async_trait::async_trait]
impl ratchet_core::ports::Clock for CountingClock {
    fn now(&self) -> i64 {
        self.inner.now()
    }

    async fn sleep(
        &self,
        ms: u64,
        _c: Option<&ratchet_core::ports::CancellationToken>,
    ) -> ratchet_core::ports::PortResult<()> {
        self.inner.advance(ms as i64);
        if self.sleeps.fetch_add(1, Ordering::SeqCst) + 1 == 3 {
            self.cancel.cancel();
        }
        Ok(())
    }
}

#[tokio::test]
async fn run_ticks_on_the_injected_clock_until_cancelled() {
    let sleeps = Arc::new(AtomicUsize::new(0));
    let cancel = ratchet_core::ports::CancellationToken::new();
    let (s2, c2) = (sleeps.clone(), cancel.clone());
    let mut h = Harness::new(HarnessOptions {
        hourly: Some(3.6),
        wrap_clock: Some(Arc::new(move |inner| {
            Arc::new(CountingClock {
                inner,
                sleeps: s2.clone(),
                cancel: c2.clone(),
            })
        })),
        ..Default::default()
    })
    .await;
    let b = begun(&h).await;
    h.engine.run(Some(cancel)).await;
    assert_eq!(sleeps.load(Ordering::SeqCst), 3);
    let spends: Vec<f64> = h
        .events()
        .iter()
        .filter_map(|e| match e {
            EngineEvent::Spend { total_usd, .. } => Some(*total_usd),
            _ => None,
        })
        .collect();
    assert!(spends.len() >= 2);
    assert!(spends.windows(2).all(|w| w[0] <= w[1]));
    assert!(h.ledger.total_spend(&oid(&b.id)).await.unwrap() > 0.0);
}

#[tokio::test]
async fn a_dropped_subscriber_does_not_disturb_the_engine_and_late_subscribers_see_later_events() {
    let h = Harness::new(Default::default()).await;
    let mut early = h.engine.subscribe();
    let dropped = h.engine.subscribe();
    drop(dropped);
    let b = begun(&h).await;
    let mut seen = Vec::new();
    while let Ok(e) = early.try_recv() {
        seen.push(e);
    }
    assert!(types(&seen).contains(&"optimization"));
    let mut late = h.engine.subscribe();
    h.engine.stop(&b.id, None).await.unwrap();
    assert!(late.try_recv().is_ok());
}

#[tokio::test]
async fn exposes_its_mode() {
    let h = Harness::new(Default::default()).await;
    assert_eq!(h.engine.mode().to_string(), "simulation");
}
