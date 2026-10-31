//! `packages/runtime/src/summary.test.ts`

use ratchet_core::config::{Objective, ValueModel};
use ratchet_core::enums::{
    AgentKind, CandidateVerdict, Direction, OptimizationStatus, SpendCategory, ValueBasis,
    ValuePeriod,
};
use ratchet_core::ids::OptimizationId;
use ratchet_core::rows::BenchStats;
use ratchet_core::{estimate_value, return_on_spend};
use ratchet_engine::runtime::summary::{
    SimulationSummary, SummaryInput, build_simulation_summary, pr_to_path, write_simulation_summary,
};
use ratchet_ledger::{
    AddAgentInput, CreateOptimizationInput, Ledger, LedgerOptions, RecordBaselineInput,
    RecordSpendInput, SubmitCandidateInput,
};

async fn ledger_in(dir: &std::path::Path) -> Ledger {
    Ledger::open(dir.join("ratchet.db"), LedgerOptions::default())
        .await
        .unwrap()
}

fn new_opt(id: Option<&str>, budget: f64, limit: i64) -> CreateOptimizationInput {
    let mut i = CreateOptimizationInput::new(
        "a/b",
        "main",
        "p95",
        Direction::Minimize,
        0.03,
        budget,
        limit,
        "v",
    );
    i.id = id.map(OptimizationId::new);
    i
}

/// A finished run: baseline 100, one verified win (80), one rejected, one blocked, some spend.
async fn finished_run(
    ledger: &Ledger,
    dir: &std::path::Path,
) -> (
    SimulationSummary,
    ratchet_core::rows::OptimizationSummary,
    ratchet_core::rows::SpendBreakdown,
    Vec<CandidateVerdict>,
) {
    let opt = ledger
        .create_optimization(new_opt(Some("opt-1"), 50.0, 600))
        .await
        .unwrap();
    ledger
        .record_baseline(RecordBaselineInput {
            optimization_id: opt.id.clone(),
            samples: vec![99.0, 100.0, 101.0],
            noise_floor: 1.0,
            stats: BenchStats::new(100.0),
            value: None,
            recorded_at: None,
        })
        .await
        .unwrap();
    let agent = ledger
        .add_agent(AddAgentInput::new(
            opt.id.clone(),
            AgentKind::ClaudeCode,
            "ratchet/opt-1/cc",
        ))
        .await
        .unwrap();
    let win = ledger
        .submit_candidate(SubmitCandidateInput::new(
            opt.id.clone(),
            agent.id.clone(),
            "aaa",
        ))
        .await
        .unwrap();
    ledger
        .set_verdict(&win.id, CandidateVerdict::Verified, &[], None)
        .await
        .unwrap();
    ledger
        .move_ratchet(&opt.id, &win.id, 80.0, None)
        .await
        .unwrap();
    let b = ledger
        .submit_candidate(SubmitCandidateInput::new(
            opt.id.clone(),
            agent.id.clone(),
            "bbb",
        ))
        .await
        .unwrap();
    ledger
        .set_verdict(&b.id, CandidateVerdict::Rejected, &["no gain".into()], None)
        .await
        .unwrap();
    let c = ledger
        .submit_candidate(SubmitCandidateInput::new(
            opt.id.clone(),
            agent.id.clone(),
            "ccc",
        ))
        .await
        .unwrap();
    ledger
        .set_verdict(
            &c.id,
            CandidateVerdict::Blocked,
            &["protected path".into()],
            None,
        )
        .await
        .unwrap();
    ledger
        .record_spend(RecordSpendInput::new(
            opt.id.clone(),
            SpendCategory::AgentRunner,
            0.25,
            "ec2:i-1",
        ))
        .await
        .unwrap();
    ledger
        .record_spend(RecordSpendInput::new(
            opt.id.clone(),
            SpendCategory::Tokens,
            0.75,
            "agent",
        ))
        .await
        .unwrap();
    let mut actual = RecordSpendInput::new(
        opt.id.clone(),
        SpendCategory::AgentRunner,
        -0.05,
        "cost-explorer",
    );
    actual.estimated = Some(false);
    ledger.record_spend(actual).await.unwrap();
    ledger
        .set_optimization_status(&opt.id, OptimizationStatus::Completed, None)
        .await
        .unwrap();

    let summary = ledger
        .list_optimizations(None)
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.id == opt.id)
        .unwrap();
    let objective = Objective {
        name: None,
        metric: "p95".into(),
        direction: Direction::Minimize,
        min_gain: None,
        value: Some(ValueModel {
            usd_per_unit: 1000.0,
            period: ValuePeriod::Month,
            basis: ValueBasis::Estimated,
            unit: Some("ms".into()),
            note: None,
        }),
    };
    let value = estimate_value(
        &objective,
        summary.baseline_value.unwrap(),
        summary.best_value.unwrap(),
    );
    let spend = ledger.spend_breakdown(&opt.id).await.unwrap();
    let verdicts: Vec<CandidateVerdict> = ledger
        .candidates_with_results(&opt.id)
        .await
        .unwrap()
        .iter()
        .map(|c| c.candidate.verdict)
        .collect();
    let roi = value
        .as_ref()
        .and_then(|v| return_on_spend(v.usd_per_year, summary.spend_usd));
    let pr = format!("file://{}", dir.join("prs").join("opt-1.md").display());
    let s = build_simulation_summary(SummaryInput {
        project: "search".into(),
        pack: Some("perf".into()),
        objective: "p95 latency".into(),
        summary: &summary,
        candidates: verdicts.clone(),
        spend: &spend,
        value,
        roi,
        pr: Some(pr),
        work_root: dir.join(".ratchet-sim-x").display().to_string(),
        db: dir.join("ratchet.db").display().to_string(),
    });
    (s, summary, spend, verdicts)
}

#[tokio::test]
async fn captures_project_baseline_best_gain_verdict_counts_spend_value_roi_and_the_pr_path() {
    let dir = tempfile::tempdir().unwrap();
    let ledger = ledger_in(dir.path()).await;
    let (s, ..) = finished_run(&ledger, dir.path()).await;
    assert_eq!(s.project, "search");
    assert_eq!(s.pack.as_deref(), Some("perf"));
    assert_eq!(s.optimization_id, "opt-1");
    assert_eq!(s.objective, "p95 latency");
    assert_eq!(s.metric, "p95");
    assert_eq!(s.status, "completed");
    assert_eq!(s.baseline, Some(100.0));
    assert_eq!(s.best, Some(80.0));
    assert_eq!(s.wins, 1);
    assert_eq!(s.candidates, 3);
    let v: Vec<(&str, u64)> = s.verdicts.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    assert_eq!(
        v,
        [
            ("pending", 0),
            ("verified", 1),
            ("rejected", 1),
            ("failed_confirm", 0),
            ("blocked", 1)
        ]
    );
    assert!((s.spend_usd - 0.95).abs() < 1e-9);
    assert!((s.spend.estimated_usd - 1.0).abs() < 1e-9);
    assert!((s.spend.actual_usd - -0.05).abs() < 1e-9);
    let by: std::collections::BTreeMap<_, _> = s
        .spend
        .by_category
        .iter()
        .map(|(k, v)| (k.as_str(), *v))
        .collect();
    for (k, want) in [
        ("agent_runner", 0.2),
        ("tokens", 0.75),
        ("experiment", 0.0),
        ("bench_runner", 0.0),
        ("storage_egress", 0.0),
    ] {
        assert!((by[k] - want).abs() < 1e-9, "{k}: {}", by[k]);
    }
    let value = s.value.as_ref().unwrap();
    assert_eq!(value.improvement, 20.0);
    assert_eq!(value.unit.as_deref(), Some("ms"));
    assert_eq!(value.usd_per_year, 240_000.0);
    assert_eq!(value.basis, "estimated");
    assert_eq!(
        s.pr_path.as_deref(),
        Some(dir.path().join("prs").join("opt-1.md").to_str().unwrap())
    );
    assert_eq!(s.db, dir.path().join("ratchet.db").display().to_string());
    assert!((s.gain_pct.unwrap() - 20.0).abs() < 1e-9);
    assert!((value.return_on_spend.unwrap() - 240_000.0 / 0.95).abs() < 1e-6);
    assert!(s.pr.as_deref().unwrap().starts_with("file://"));
}

#[tokio::test]
async fn handles_a_run_with_no_win_no_value_model_and_no_pr() {
    let dir = tempfile::tempdir().unwrap();
    let ledger = ledger_in(dir.path()).await;
    let opt = ledger
        .create_optimization(new_opt(None, 5.0, 60))
        .await
        .unwrap();
    let summary = ledger
        .list_optimizations(None)
        .await
        .unwrap()
        .into_iter()
        .find(|s| s.id == opt.id)
        .unwrap();
    let spend = ledger.spend_breakdown(&opt.id).await.unwrap();
    let s = build_simulation_summary(SummaryInput {
        project: "forecast".into(),
        pack: None,
        objective: "p95".into(),
        summary: &summary,
        candidates: vec![],
        spend: &spend,
        value: None,
        roi: None,
        pr: None,
        work_root: "w".into(),
        db: "d".into(),
    });
    assert_eq!(s.pack, None);
    assert_eq!((s.baseline, s.best, s.gain_pct), (None, None, None));
    assert_eq!(s.wins, 0);
    assert!(s.value.is_none() && s.pr.is_none() && s.pr_path.is_none());
    assert_eq!(s.spend_usd, 0.0);
    assert_eq!(
        s.verdicts.values().copied().collect::<Vec<_>>(),
        [0, 0, 0, 0, 0]
    );
}

#[test]
fn pr_to_path_converts_file_urls_and_passes_other_urls_through() {
    assert_eq!(pr_to_path("file:///tmp/a%20b/pr.md"), "/tmp/a b/pr.md");
    assert_eq!(
        pr_to_path("https://github.com/a/b/pull/7"),
        "https://github.com/a/b/pull/7"
    );
}

#[tokio::test]
async fn write_simulation_summary_writes_pretty_json_creating_parent_directories_and_leaves_no_temp_file_behind()
 {
    let dir = tempfile::tempdir().unwrap();
    let ledger = ledger_in(dir.path()).await;
    let (summary, ..) = finished_run(&ledger, dir.path()).await;
    let target = dir.path().join("out").join("deep").join("summary.json");
    let written = write_simulation_summary(&target, &summary).unwrap();
    assert_eq!(written, target);
    let text = std::fs::read_to_string(&target).unwrap();
    assert!(text.ends_with('\n'));
    assert!(text.contains("\n  \"project\": \"search\""));
    assert_eq!(
        serde_json::from_str::<serde_json::Value>(&text).unwrap(),
        serde_json::to_value(&summary).unwrap()
    );
    let names: Vec<_> = std::fs::read_dir(target.parent().unwrap())
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert_eq!(names, ["summary.json"]);
    // overwriting works
    let mut again = summary.clone();
    again.project = "forecast".into();
    write_simulation_summary(&target, &again).unwrap();
    let back: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&target).unwrap()).unwrap();
    assert_eq!(back["project"], "forecast");
}
