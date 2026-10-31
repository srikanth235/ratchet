//! The mappers over the ledger (ported from `packages/desktop/src/main/mappers.test.ts`): the numbers on the
//! screens, computed from rows the test writes itself.

use ratchet_core::config::GuardrailResult;
use ratchet_core::enums::{
    AgentKind, BenchPhase, CandidateVerdict, Direction, OptimizationStatus, ValueBasis, ValuePeriod,
};
use ratchet_core::ids::{AgentId, OptimizationId};
use ratchet_core::ports::FileDiff;
use ratchet_core::rows::BenchStats;
use ratchet_ledger::{
    AddAgentInput, CreateOptimizationInput, RecordBaselineInput, RecordBenchResultInput,
    RecordSpendInput, SubmitCandidateInput, ValueInput,
};
use ratchetd::api::*;
use ratchetd::service::AppService;
use ratchetd::sim::{SimOptions, SimService};

const DAY: i64 = 86_400_000;

async fn empty() -> SimService {
    SimService::new(SimOptions {
        seed_demo: false,
        auto_run: false,
        ..SimOptions::default()
    })
    .await
    .unwrap()
}

/// An optimization written by hand: `wins` are `(days ago, value)`; spend `usd` at the start.
async fn hand_made(
    sim: &SimService,
    id: &str,
    valued: bool,
    started_days_ago: i64,
    wins: &[(i64, f64)],
    usd: f64,
) {
    let l = sim.ledger();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let start = now - started_days_ago * DAY;
    let mut c = CreateOptimizationInput::new(
        "acme/x",
        "main",
        "lat.p95",
        Direction::Minimize,
        0.03,
        100.0,
        3600,
        "version: 0\n",
    );
    c.id = Some(OptimizationId::new(id));
    c.created_at = Some(start - 1000);
    if valued {
        c.value = Some(ValueInput {
            usd_per_unit: 10.0,
            period: ValuePeriod::Month,
            basis: ValueBasis::Measured,
            unit: Some("ms".into()),
        });
    }
    l.create_optimization(c).await.unwrap();
    let oid = OptimizationId::new(id);
    l.set_optimization_status(&oid, OptimizationStatus::Running, Some(start))
        .await
        .unwrap();
    l.record_baseline(RecordBaselineInput {
        optimization_id: oid.clone(),
        samples: vec![100.0],
        noise_floor: 1.0,
        stats: BenchStats::new(100.0),
        value: None,
        recorded_at: Some(start),
    })
    .await
    .unwrap();
    let mut a = AddAgentInput::new(oid.clone(), AgentKind::Codex, "b");
    a.id = Some(AgentId::new(format!("{id}_a")));
    let agent = l.add_agent(a).await.unwrap();
    for (n, (days_ago, value)) in wins.iter().enumerate() {
        let at = now - days_ago * DAY;
        let cand = l
            .submit_candidate(SubmitCandidateInput {
                id: None,
                optimization_id: oid.clone(),
                agent_id: agent.id.clone(),
                sha: format!("{n:040}"),
                message: Some(format!("win {n}")),
                submitted_at: Some(at),
            })
            .await
            .unwrap();
        l.record_bench_result(RecordBenchResultInput {
            candidate_id: cand.id.clone(),
            phase: BenchPhase::Bench,
            samples: vec![*value],
            stats: BenchStats::new(*value),
            guardrails: vec![GuardrailResult {
                name: "g".into(),
                passed: true,
                detail: None,
            }],
            started_at: at,
            finished_at: at,
        })
        .await
        .unwrap();
        l.set_verdict(
            &cand.id,
            CandidateVerdict::Verified,
            &["ok".into()],
            Some(at),
        )
        .await
        .unwrap();
        l.move_ratchet(&oid, &cand.id, *value, Some(at))
            .await
            .unwrap();
    }
    let mut s = RecordSpendInput::new(oid, ratchet_core::SpendCategory::Tokens, usd, "test");
    s.at = Some(start);
    l.record_spend(s).await.unwrap();
}

fn kpi<'a>(s: &'a OptimizationsSnapshot, id: &str) -> &'a Kpi {
    s.kpis
        .iter()
        .find(|k| k.id == id)
        .unwrap_or_else(|| panic!("no {id} KPI"))
}

#[tokio::test]
async fn range_kpis_carry_change_chips_against_the_previous_equal_window() {
    let sim = empty().await;
    // 30-day windows: now-30d..now has 3 wins, now-60d..now-30d has 1; spend sits at the start of the run
    hand_made(
        &sim,
        "o1",
        true,
        10,
        &[(8, 90.0), (5, 80.0), (2, 70.0)],
        6.0,
    )
    .await;
    hand_made(&sim, "o2", false, 45, &[(40, 90.0)], 3.0).await;
    let s = sim.list_optimizations(KpiRange::Days30).await.unwrap();

    let wins = kpi(&s, "wins");
    assert_eq!(
        (wins.value.as_str(), wins.unit.as_deref()),
        ("3", Some("last 30d"))
    );
    let ch = wins.change.as_ref().unwrap();
    assert_eq!((ch.text.as_str(), ch.tone), ("+2", Tone::Good));
    assert_eq!(wins.foot, "across 1 optimization");
    assert_eq!(wins.trend.len(), 10);
    assert_eq!(
        *wins.trend.last().unwrap(),
        3.0,
        "the sparkline ends at the total"
    );

    let gain = kpi(&s, "gain");
    assert_eq!(
        gain.value, "30",
        "the one optimization that ratcheted is 30% better than its baseline"
    );
    assert_eq!(gain.unit.as_deref(), Some("% better"));
    let g = gain.change.as_ref().unwrap();
    assert_eq!(g.text, "+20 pts", "vs 10% in the previous window");

    let spend = kpi(&s, "spend");
    assert_eq!(spend.value, "$6.0");
    let sc = spend.change.as_ref().unwrap();
    assert_eq!(
        (sc.text.as_str(), sc.tone),
        ("+100%", Tone::Neutral),
        "spend is neutral: more is not good or bad"
    );

    let cpw = kpi(&s, "cpw");
    assert_eq!(cpw.value, "$2.0");
    assert_eq!(
        cpw.change.as_ref().unwrap().tone,
        Tone::Good,
        "cost per win fell from $3 to $2"
    );

    // the value KPI: o1 only; (100-70) ms * $10 per month * 12 = $3,600 / yr on $6 -> 600x
    let v = kpi(&s, "value");
    assert_eq!(v.value, "$3,600");
    assert!(
        v.foot.contains("600.0\u{d7} ROI") || v.foot.contains("600\u{d7} ROI"),
        "{}",
        v.foot
    );
    assert!(v.foot.ends_with("measured"));
}

#[tokio::test]
async fn kpis_omit_chips_without_a_previous_window_and_a_longer_range_counts_older_wins() {
    let sim = empty().await;
    hand_made(&sim, "o1", false, 10, &[(8, 90.0)], 2.0).await;
    hand_made(&sim, "o2", false, 45, &[(40, 90.0), (39, 80.0)], 2.0).await;
    let short = sim.list_optimizations(KpiRange::Days7).await.unwrap();
    assert_eq!(kpi(&short, "wins").value, "0");
    assert_eq!(kpi(&short, "wins").foot, "none in this range");
    assert_eq!(kpi(&short, "gain").value, "\u{2014}");
    assert_eq!(kpi(&short, "gain").trend, Vec::<f64>::new());
    assert_eq!(kpi(&short, "cpw").value, "\u{2014}");
    // 90 days: 3 wins, and nothing happened before day 90, so there is nothing to compare with
    let long = sim.list_optimizations(KpiRange::Days90).await.unwrap();
    assert_eq!(kpi(&long, "wins").value, "3");
    assert!(kpi(&long, "wins").change.is_none());
    assert!(kpi(&long, "spend").change.is_none());
    assert!(kpi(&long, "gain").change.is_none());
}

#[tokio::test]
async fn the_value_kpi_exists_only_when_some_optimization_has_a_value_model() {
    let sim = empty().await;
    let none = sim.list_optimizations(KpiRange::Days30).await.unwrap();
    assert_eq!(none.kpis.len(), 4, "no value card");
    assert_eq!(kpi(&none, "wins").value, "0");
    assert_eq!(kpi(&none, "spend").value, "$0.0");
    assert!(
        none.optimizations.is_empty()
            && none.featured_id.is_none()
            && none.featured_series.is_none()
    );
    hand_made(&sim, "plain", false, 3, &[(2, 90.0)], 1.0).await;
    let plain = sim.list_optimizations(KpiRange::Days30).await.unwrap();
    assert_eq!(plain.kpis.len(), 4);
    hand_made(&sim, "valued", true, 3, &[(2, 90.0)], 1.0).await;
    let valued = sim.list_optimizations(KpiRange::Days30).await.unwrap();
    assert_eq!(valued.kpis.len(), 5);
    assert_eq!(
        valued.featured_id.as_deref(),
        Some("valued"),
        "the newest running optimization is featured"
    );
}

#[tokio::test]
async fn the_improvement_is_valued_from_the_ledger_columns() {
    let sim = SimService::manual().await.unwrap();
    let d = sim.get_optimization("opt_demo_search").await.unwrap();
    let v = d.summary.value.unwrap();
    // 182 -> 131 ms at $1200 per ms per year
    assert_eq!(
        (v.usd_per_unit, v.unit.as_deref(), v.period, v.basis),
        (1200.0, Some("ms"), ValuePeriod::Year, ValueBasis::Estimated)
    );
    assert!((v.usd_per_year - 51.0 * 1200.0).abs() < 1e-6);
    let ros = v.return_on_spend.unwrap();
    assert!((ros - v.usd_per_year / d.summary.spend_usd).abs() < 1e-6);
    // 41.2 -> 58.9 k/s at $400 per k/s per year
    let ingest = sim
        .get_optimization("opt_demo_ingest")
        .await
        .unwrap()
        .summary
        .value
        .unwrap();
    assert_eq!(
        (ingest.usd_per_unit, ingest.unit.as_deref()),
        (400.0, Some("k/s"))
    );
    assert!((ingest.usd_per_year - 17.7 * 400.0).abs() < 1e-6);
    // a run without a value model, or without an improvement, has no value
    assert!(
        sim.get_optimization("opt_demo_web")
            .await
            .unwrap()
            .summary
            .value
            .is_none()
    );
}

#[tokio::test]
async fn activity_is_newest_first_and_a_verified_verdict_shows_once() {
    let sim = SimService::manual().await.unwrap();
    let all = sim
        .recent_activity(Some("opt_demo_search".into()), 50)
        .await
        .unwrap();
    assert!(all.windows(2).all(|w| w[0].at_ms >= w[1].at_ms));
    // 11 verdicts, 5 of them verified: those are shown as the ratchet moves, so 11 rows and no duplicates
    assert_eq!(all.len(), 11);
    let wins: Vec<&ActivityVm> = all
        .iter()
        .filter(|a| a.text.starts_with("Verified win"))
        .collect();
    assert_eq!(wins.len(), 5);
    assert!(
        wins.iter()
            .all(|a| a.tone == ActivityTone::Mint && a.id.starts_with("m-"))
    );
    assert!(
        all.iter()
            .all(|a| !a.text.starts_with("Candidate verified"))
    );
    assert!(
        wins[0].text.contains("Binary-search the sorted vocabulary")
            && wins[0].text.ends_with("\u{2192} 131 ms"),
        "{}",
        wins[0].text
    );
    let rose: Vec<&str> = all
        .iter()
        .filter(|a| a.tone == ActivityTone::Rose)
        .map(|a| a.text.as_str())
        .collect();
    assert_eq!(rose.len(), 2, "failed confirm and blocked");
    assert!(
        rose.iter()
            .any(|t| t.starts_with("Failed confirm on the hidden trace"))
    );
    assert!(
        rose.iter()
            .any(|t| t.starts_with("Blocked: protected path changed"))
    );
    let limited = sim.recent_activity(None, 3).await.unwrap();
    assert_eq!(limited.len(), 3);
}

#[tokio::test]
async fn the_winning_change_comes_from_a_diff_or_says_why_there_is_none() {
    let sim = SimService::manual().await.unwrap();
    let none = sim
        .views()
        .winning_change("opt_demo_web", None, None)
        .await
        .unwrap();
    assert_eq!(
        (
            none.commits,
            none.sha.as_deref(),
            none.unavailable.as_deref()
        ),
        (0, None, Some("No verified win yet."))
    );
    let nodiff = sim
        .views()
        .winning_change("opt_demo_search", None, Some("git failed".into()))
        .await
        .unwrap();
    assert_eq!(
        (nodiff.commits, nodiff.unavailable.as_deref()),
        (5, Some("git failed"))
    );
    assert_eq!(nodiff.sha.as_ref().unwrap().len(), 7, "a short sha");

    let patch = "@@ -1,3 +1,4 @@\n keep\n-old\n+new\n+newer\n\\ No newline at end of file\n tail\n@@ -20 +21 @@\n-second hunk\n";
    let files = vec![
        FileDiff {
            path: "small.ts".into(),
            additions: 1,
            deletions: 0,
            binary: false,
            patch: "@@ -1 +1,2 @@\n a\n+b\n".into(),
        },
        FileDiff {
            path: "big.ts".into(),
            additions: 40,
            deletions: 2,
            binary: false,
            patch: patch.into(),
        },
        FileDiff {
            path: "logo.png".into(),
            additions: 0,
            deletions: 0,
            binary: true,
            patch: String::new(),
        },
    ];
    let wc = sim
        .views()
        .winning_change("opt_demo_search", Some(files), None)
        .await
        .unwrap();
    assert!(wc.unavailable.is_none());
    assert_eq!((wc.files.len(), wc.additions, wc.deletions), (3, 41, 2));
    assert!(wc.files[2].binary);
    let ex = wc.excerpt.unwrap();
    assert_eq!(ex.path, "big.ts", "the most-changed file");
    let kinds: Vec<(DiffLineKind, &str)> =
        ex.lines.iter().map(|l| (l.kind, l.text.as_str())).collect();
    assert_eq!(
        kinds,
        [
            (DiffLineKind::Hunk, "@@ -1,3 +1,4 @@"),
            (DiffLineKind::Ctx, "keep"),
            (DiffLineKind::Del, "old"),
            (DiffLineKind::Add, "new"),
            (DiffLineKind::Add, "newer"),
            (DiffLineKind::Ctx, "tail"),
        ],
        "first hunk only, no 'no newline' marker"
    );
}

#[tokio::test]
async fn the_hypothesis_is_the_latest_agent_message_not_the_users() {
    let sim = SimService::manual().await.unwrap();
    let agents = sim
        .get_optimization("opt_demo_search")
        .await
        .unwrap()
        .agents;
    let a = &agents[0];
    let before = a.hypothesis.clone().unwrap();
    assert_eq!(
        before,
        "Binary-search the sorted vocabulary for prefix terms"
    );
    sim.message_agent(&a.id, "please stop guessing")
        .await
        .unwrap();
    let after = sim
        .get_optimization("opt_demo_search")
        .await
        .unwrap()
        .agents
        .remove(0);
    assert_eq!(
        after.hypothesis.as_deref(),
        Some(before.as_str()),
        "user messages are ignored"
    );
    assert!(after.last_event_id > a.last_event_id);
    assert!(
        a.recent.iter().all(|r| r.label.split(' ').count() == 2),
        "sha and verdict"
    );
}

#[tokio::test]
async fn the_pipeline_follows_the_newest_candidate() {
    let sim = empty().await;
    hand_made(&sim, "p", false, 1, &[], 0.0).await;
    let idle = sim.get_optimization("p").await.unwrap().pipeline;
    assert!(idle.candidate.is_none());
    assert!(idle.stages.iter().all(|s| s.state == StageState::Idle));
    assert_eq!(idle.stages[0].detail, "waiting for a candidate");

    // a submitted, undecided candidate: checked out, guardrails running
    let l = sim.ledger();
    let cand = l
        .submit_candidate(SubmitCandidateInput::new(
            "p",
            "p_a",
            "abcdef0123456789abcdef0123456789abcdef01",
        ))
        .await
        .unwrap();
    let st = sim.get_optimization("p").await.unwrap().pipeline;
    assert_eq!(st.candidate.as_ref().unwrap().sha, "abcdef0");
    let by = |id: StageId| st.stages.iter().find(|s| s.id == id).unwrap();
    assert_eq!(by(StageId::Checkout).state, StageState::Done);
    assert_eq!(by(StageId::Guardrails).state, StageState::Active);
    assert_eq!(by(StageId::Verdict).state, StageState::Active);
    assert_eq!(st.queued, 0);

    // a bench result with a failed guardrail, then the verdict: blocked
    l.record_bench_result(RecordBenchResultInput {
        candidate_id: cand.id.clone(),
        phase: BenchPhase::Bench,
        samples: vec![90.0, 91.0, 89.0],
        stats: BenchStats::new(90.0),
        guardrails: vec![GuardrailResult {
            name: "pnpm test".into(),
            passed: false,
            detail: None,
        }],
        started_at: 1,
        finished_at: 2,
    })
    .await
    .unwrap();
    l.set_verdict(
        &cand.id,
        CandidateVerdict::Blocked,
        &["guardrail failed: pnpm test".into()],
        None,
    )
    .await
    .unwrap();
    let st = sim.get_optimization("p").await.unwrap().pipeline;
    let by = |id: StageId| st.stages.iter().find(|s| s.id == id).unwrap();
    assert_eq!(
        (
            by(StageId::Guardrails).state,
            by(StageId::Guardrails).detail.as_str()
        ),
        (StageState::Failed, "pnpm test")
    );
    assert_eq!(by(StageId::Bench).label, "Bench \u{d7}3");
    assert_eq!(by(StageId::Bench).detail, "median 90");
    assert_eq!(by(StageId::Confirm).detail, "skipped");
    assert_eq!(
        (
            by(StageId::Verdict).state,
            by(StageId::Verdict).detail.as_str()
        ),
        (StageState::Failed, "blocked")
    );
}

#[tokio::test]
async fn reports_say_when_there_is_nothing_yet() {
    let sim = empty().await;
    hand_made(&sim, "r", false, 1, &[], 0.0).await;
    let r = sim.report("r").await.unwrap();
    assert!(r.markdown.contains("_None yet._"));
    assert!(
        r.markdown
            .contains("with 0 verified wins out of 0 candidates")
    );
    assert!(r.markdown.contains("Status: running"));
}
