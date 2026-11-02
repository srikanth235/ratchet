//! P7: the cost split and the value story. A key-billed and a subscription optimization side by side: real dollars only in
//! `spent` and in the return, tokens on a plan counted separately, delivered (merged) vs pending (open pull request) value,
//! the forecast, the chart, the empty states, and Home and Overview telling the same story.

use std::sync::Arc;

use ratchet_core::config::GuardrailResult;
use ratchet_core::enums::{
    AgentKind, BenchPhase, Billing, CandidateVerdict, Direction, OptimizationStatus, ScheduleKind,
    SpendCategory, ValueBasis, ValuePeriod,
};
use ratchet_core::ids::{AgentId, OptimizationId};
use ratchet_core::ports::Clock;
use ratchet_core::ports::mock::ManualClock;
use ratchet_core::rows::BenchStats;
use ratchet_ledger::{
    AddAgentInput, CreateOptimizationInput, RecordBaselineInput, RecordBenchResultInput,
    RecordSpendInput, SubmitCandidateInput, ValueInput,
};
use ratchetd::api::*;
use ratchetd::service::AppService;
use ratchetd::sim::{SimOptions, SimService};
use ratchetd::views::{LedgerViews, PrMergedLookup, PrUrlLookup};

/// 2026-10-30T12:00:00Z
const NOW: i64 = 1_793_361_600_000;
const DAY: i64 = 86_400_000;

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-6, "{a} is not {b}");
}

async fn sim(seed: bool, story: bool) -> SimService {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(NOW));
    SimService::new(SimOptions {
        seed_demo: seed,
        seed_value_story: story,
        auto_run: false,
        resume_runs: false,
        github_flow_ms: 0,
        clock: Some(clock),
        ..SimOptions::default()
    })
    .await
    .unwrap()
}

/// One optimization: a $/unit value model, a baseline of 100 (lower is better), verified wins `(days ago, new best)`, a schedule.
async fn make(
    sim: &SimService,
    id: &str,
    agent: AgentKind,
    usd_per_unit: f64,
    budget: f64,
    schedule: Option<ScheduleKind>,
    wins: &[(i64, f64)],
) {
    let l = sim.ledger();
    let start = NOW - 30 * DAY;
    let mut c = CreateOptimizationInput::new(
        "acme/x",
        "main",
        "lat.p95",
        Direction::Minimize,
        0.03,
        budget,
        3600,
        "version: 0\n",
    );
    c.id = Some(OptimizationId::new(id));
    c.name = Some(id.to_owned());
    c.created_at = Some(start - 1000);
    c.schedule = schedule;
    c.value = Some(ValueInput {
        usd_per_unit,
        period: ValuePeriod::Year,
        basis: ValueBasis::Estimated,
        unit: Some("ms".into()),
        note: Some("ms saved x volume".into()),
    });
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
    let mut a = AddAgentInput::new(oid.clone(), agent, "b");
    a.id = Some(AgentId::new(format!("{id}_a")));
    let agent = l.add_agent(a).await.unwrap();
    for (n, (days_ago, value)) in wins.iter().enumerate() {
        let at = NOW - days_ago * DAY;
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
    l.set_optimization_status(&oid, OptimizationStatus::Completed, Some(NOW - DAY))
        .await
        .unwrap();
}

async fn spend(
    sim: &SimService,
    id: &str,
    category: SpendCategory,
    usd: f64,
    billing: Billing,
    tokens: u64,
    days_ago: i64,
) {
    let mut s = RecordSpendInput::new(OptimizationId::new(id), category, usd, "test");
    s.at = Some(NOW - days_ago * DAY);
    s.billing = Some(billing);
    s.input_tokens = tokens * 9 / 10;
    s.output_tokens = tokens - tokens * 9 / 10;
    sim.ledger().record_spend(s).await.unwrap();
}

/// `merged` / `open` name optimizations; the rest have no pull request.
fn views(
    sim: &SimService,
    open: &'static [&'static str],
    merged: &'static [&'static str],
) -> LedgerViews {
    let url: PrUrlLookup = Arc::new(move |id| {
        (open.contains(&id) || merged.contains(&id))
            .then(|| format!("https://github.com/acme/x/pull/{id}"))
    });
    let is_merged: PrMergedLookup = Arc::new(move |id| merged.contains(&id));
    LedgerViews::new(sim.ledger().clone(), Arc::new(ManualClock::new(NOW)), url)
        .with_pr_merged(is_merged)
}

/// A key-billed checkout optimization (merged), a subscription search optimization (open pull request) and one with wins nobody
/// has opened a pull request for yet.
async fn mixed() -> SimService {
    let sim = sim(false, false).await;
    // $10 of p95 gained at $1,000 per ms a year = $10,000 a year, merged
    make(
        &sim,
        "opt_key",
        AgentKind::ClaudeCode,
        1000.0,
        100.0,
        Some(ScheduleKind::Nightly),
        &[(20, 90.0)],
    )
    .await;
    spend(
        &sim,
        "opt_key",
        SpendCategory::BenchRunner,
        30.0,
        Billing::Api,
        0,
        25,
    )
    .await;
    spend(
        &sim,
        "opt_key",
        SpendCategory::Tokens,
        50.0,
        Billing::Api,
        1_100_000,
        25,
    )
    .await;
    // 6 ms at $500 = $3,000 a year, in an open pull request; the agent runs on a login: tokens, no dollars
    make(
        &sim,
        "opt_plan",
        AgentKind::Codex,
        500.0,
        50.0,
        Some(ScheduleKind::Weekly),
        &[(10, 94.0)],
    )
    .await;
    spend(
        &sim,
        "opt_plan",
        SpendCategory::BenchRunner,
        20.0,
        Billing::Api,
        0,
        12,
    )
    .await;
    spend(
        &sim,
        "opt_plan",
        SpendCategory::Tokens,
        9.0,
        Billing::Subscription,
        2_400_000,
        12,
    )
    .await;
    // 2 ms at $100 = $200 a year, no pull request yet
    make(
        &sim,
        "opt_ready",
        AgentKind::ClaudeCode,
        100.0,
        40.0,
        None,
        &[(5, 98.0)],
    )
    .await;
    spend(
        &sim,
        "opt_ready",
        SpendCategory::BenchRunner,
        10.0,
        Billing::Api,
        0,
        6,
    )
    .await;
    sim
}

#[tokio::test]
async fn key_billed_and_subscription_optimizations_split_spend_into_real_dollars_and_plan_tokens() {
    let sim = mixed().await;
    let o = views(&sim, &["opt_plan"], &["opt_key"])
        .overview(OverviewRange::All)
        .await
        .unwrap();
    // AWS $30 + $20 + $10, agents on a key $50; the plan's 2.4M tokens are not dollars
    close(o.spent.aws_usd, 60.0);
    close(o.spent.agents_api_usd, 50.0);
    close(o.spent.usd, 110.0);
    assert_eq!(o.spent.included_tokens, 2_400_000);
    assert_eq!(
        o.spent.detail,
        "AWS $60 \u{b7} agents $50 + 2.4M tokens in your plan"
    );
    assert_eq!(o.story.spent_usd, o.spent.usd);
    // rows: the plan's optimization spent only its AWS dollars
    let plan = o.rows.iter().find(|r| r.id == "opt_plan").unwrap();
    close(plan.spent_usd, 20.0);
    let key = o.rows.iter().find(|r| r.id == "opt_key").unwrap();
    close(key.spent_usd, 80.0);
}

#[tokio::test]
async fn return_is_delivered_value_over_real_dollars_and_pending_waits_for_the_merge() {
    let sim = mixed().await;
    let o = views(&sim, &["opt_plan"], &["opt_key"])
        .overview(OverviewRange::All)
        .await
        .unwrap();
    let s = &o.story;
    close(s.delivered.usd_per_year, 10_000.0);
    assert_eq!(s.delivered.wins, 1);
    close(s.pending.usd_per_year, 3_000.0);
    assert_eq!((s.pending.wins, s.pending.pull_requests), (1, 1));
    close(s.ready_to_ship.usd_per_year, 200.0);
    // return = delivered / real dollars: 10,000 / 110; pending and ready value is not in it
    close(s.return_x.unwrap(), 10_000.0 / 110.0);
    close(s.payback_days.unwrap(), 110.0 / 10_000.0 * 365.0);
    assert_eq!(
        s.line,
        "$10k delivered + $3k pending + $200 ready to ship \u{b7} 91\u{d7}"
    );
    assert_eq!(s.basis, ValueBasis::Estimated);

    // per row: value, spent, return, and the pending flag
    let row = |id: &str| o.rows.iter().find(|r| r.id == id).unwrap();
    let (key, plan, ready) = (row("opt_key"), row("opt_plan"), row("opt_ready"));
    assert_eq!(
        (key.value_state, key.pending),
        (OverviewValueState::Delivered, false)
    );
    assert_eq!(
        (plan.value_state, plan.pending),
        (OverviewValueState::Pending, true)
    );
    assert_eq!(
        (ready.value_state, ready.pending),
        (OverviewValueState::Ready, true)
    );
    close(key.value_usd_per_year.unwrap(), 10_000.0);
    close(key.return_x.unwrap(), 10_000.0 / 80.0);
    close(plan.return_x.unwrap(), 3_000.0 / 20.0);
    assert!(key.pull_request.as_ref().unwrap().merged);
    assert!(!plan.pull_request.as_ref().unwrap().merged);
    assert!(ready.pull_request.is_none());
    assert_eq!(key.status_label, "Merged");
    // the value model rides along, plain words included
    let model = key.value_model.as_ref().unwrap();
    assert_eq!(
        model.summary,
        "$1,000 per ms a year \u{b7} estimated \u{b7} ms saved x volume"
    );

    // before anything merges nothing is delivered and there is no return yet
    let none = views(&sim, &["opt_plan", "opt_key"], &[])
        .overview(OverviewRange::All)
        .await
        .unwrap();
    assert_eq!(none.story.delivered.usd_per_year, 0.0);
    assert!(none.story.return_x.is_none() && none.story.payback_days.is_none());
    close(none.story.pending.usd_per_year, 13_000.0);
    assert_eq!(none.story.pending.pull_requests, 2);
    assert_eq!(none.story.line, "$13k pending + $200 ready to ship");
}

#[tokio::test]
async fn the_forecast_uses_each_schedule_and_the_same_split() {
    let sim = mixed().await;
    let o = views(&sim, &["opt_plan"], &["opt_key"])
        .overview(OverviewRange::All)
        .await
        .unwrap();
    let f = &o.forecast;
    assert_eq!((f.days, f.scheduled_runs, f.unforecast), (30, 34, 0));
    // nightly (30 runs of $30 AWS + $50 on a key) and weekly (4 runs of $20 AWS + 2.4M plan tokens)
    close(f.expected_aws_usd, 30.0 * 30.0 + 4.0 * 20.0);
    close(f.expected_agents_api_usd, 30.0 * 50.0);
    close(
        f.expected_usd,
        f.expected_aws_usd + f.expected_agents_api_usd,
    );
    assert_eq!(f.expected_included_tokens, 4 * 2_400_000);
    // the ceiling is the budgets of the optimizations that ran: $100 + $50 + $40
    close(f.hard_cap_usd.unwrap(), 190.0);
    close(o.spent.budget_usd, 190.0);
    assert_eq!(
        f.line,
        "34 runs \u{b7} ~$2.5k + 9.6M tokens in your plan \u{b7} cap $190"
    );
}

#[tokio::test]
async fn a_run_never_forecasts_more_than_its_budget_and_unforecastable_schedules_say_so() {
    let sim = sim(false, false).await;
    // spent $80 over its life against a $40 budget: each future run is capped at $40 (split like the real one)
    make(
        &sim,
        "opt_a",
        AgentKind::ClaudeCode,
        10.0,
        40.0,
        Some(ScheduleKind::Nightly),
        &[(3, 95.0)],
    )
    .await;
    spend(
        &sim,
        "opt_a",
        SpendCategory::BenchRunner,
        30.0,
        Billing::Api,
        0,
        4,
    )
    .await;
    spend(
        &sim,
        "opt_a",
        SpendCategory::Tokens,
        50.0,
        Billing::Api,
        100,
        4,
    )
    .await;
    make(
        &sim,
        "opt_b",
        AgentKind::ClaudeCode,
        10.0,
        40.0,
        Some(ScheduleKind::OnPush),
        &[],
    )
    .await;
    make(
        &sim,
        "opt_c",
        AgentKind::ClaudeCode,
        10.0,
        40.0,
        Some(ScheduleKind::Once),
        &[],
    )
    .await;
    let f = views(&sim, &[], &[])
        .overview(OverviewRange::All)
        .await
        .unwrap()
        .forecast;
    assert_eq!((f.scheduled_runs, f.unforecast), (30, 1));
    close(f.expected_usd, 30.0 * 40.0);
    close(f.expected_aws_usd, 30.0 * 15.0);
    close(f.expected_agents_api_usd, 30.0 * 25.0);
    assert_eq!(f.expected_included_tokens, 0);
}

#[tokio::test]
async fn the_chart_carries_the_spend_line_and_the_staircase_steps() {
    let sim = mixed().await;
    let o = views(&sim, &["opt_plan"], &["opt_key"])
        .overview(OverviewRange::Days90)
        .await
        .unwrap();
    let pts = &o.chart;
    assert_eq!(pts.first().unwrap().at, o.from);
    assert_eq!(pts.last().unwrap().at, o.to);
    assert!(
        pts.windows(2).all(|w| w[0].at < w[1].at),
        "oldest first, one point per time"
    );
    assert!(
        pts.windows(2)
            .all(|w| w[0].spent_usd <= w[1].spent_usd + 1e-9)
    );
    assert!(
        pts.windows(2)
            .all(|w| w[0].delivered_usd_per_year <= w[1].delivered_usd_per_year)
    );
    let last = pts.last().unwrap();
    close(last.spent_usd, o.spent.usd);
    close(last.delivered_usd_per_year, o.story.delivered.usd_per_year);
    close(last.pending_usd_per_year, o.story.pending.usd_per_year);
    close(last.ready_usd_per_year, o.story.ready_to_ship.usd_per_year);
    // the staircase steps at the win, not before
    let at_win = pts.iter().find(|p| p.at == NOW - 20 * DAY).unwrap();
    close(at_win.delivered_usd_per_year, 10_000.0);
    let before = pts.iter().rev().find(|p| p.at < NOW - 20 * DAY).unwrap();
    close(before.delivered_usd_per_year, 0.0);
}

#[tokio::test]
async fn empty_states_say_whether_there_is_nothing_at_all_or_just_no_win_yet() {
    let sim = sim(false, false).await;
    let v = views(&sim, &[], &[]);
    let o = v.overview(OverviewRange::Days90).await.unwrap();
    assert_eq!((o.phase, o.empty), (OverviewPhase::Empty, true));
    let e = o.empty_state.clone().unwrap();
    assert_eq!(
        (e.title.as_str(), e.body.as_str()),
        ("No wins yet", "Start your first optimization.")
    );
    assert_eq!(o.story.line, "");
    assert_eq!(o.forecast.scheduled_runs, 0);
    assert!(ratchetd::views::overview::home_of(o.clone()).empty);

    make(
        &sim,
        "opt_new",
        AgentKind::ClaudeCode,
        10.0,
        40.0,
        None,
        &[],
    )
    .await;
    let o = v.overview(OverviewRange::Days90).await.unwrap();
    assert_eq!((o.phase, o.empty), (OverviewPhase::NoWins, false));
    assert!(o.empty_state.is_some());
    assert_eq!(o.story.line, "Nothing delivered yet");

    make(
        &sim,
        "opt_won",
        AgentKind::ClaudeCode,
        10.0,
        40.0,
        None,
        &[(2, 90.0)],
    )
    .await;
    let o = v.overview(OverviewRange::Days90).await.unwrap();
    assert_eq!(o.phase, OverviewPhase::Wins);
    assert!(o.empty_state.is_none());
}

#[tokio::test]
async fn home_and_overview_return_identical_value_numbers() {
    let sim = sim(true, true).await;
    for range in [
        OverviewRange::Days30,
        OverviewRange::Days90,
        OverviewRange::All,
    ] {
        let o = sim.overview(range).await.unwrap();
        let h = sim.home(range).await.unwrap();
        assert_eq!(h.story, o.story, "{range:?}");
        assert_eq!(h.range, o.range);
        assert_eq!(h.optimizations, o.spent.optimizations);
        // the same numbers the Overview table adds up
        close(
            o.rows.iter().map(|r| r.spent_usd).sum::<f64>(),
            o.story.spent_usd,
        );
    }
    // and the seeded mock shows every part of the story: a merged win, an open pull request, a login, a key
    let o = sim.overview(OverviewRange::Days90).await.unwrap();
    assert!(o.story.delivered.usd_per_year > 0.0, "{:?}", o.story);
    assert!(o.story.pending.usd_per_year > 0.0 && o.story.pending.pull_requests == 1);
    assert!(o.story.return_x.is_some());
    assert!(o.spent.included_tokens > 0 && o.spent.agents_api_usd > 0.0 && o.spent.aws_usd > 0.0);
    assert!(o.forecast.scheduled_runs > 0 && o.forecast.hard_cap_usd.is_some());
    let h = sim.home(OverviewRange::Days90).await.unwrap();
    assert_eq!((h.pull_requests.open, h.pull_requests.merged), (1, 1));
}

#[tokio::test]
async fn the_value_model_round_trips_and_re_values_the_wins() {
    let sim = mixed().await;
    let v = views(&sim, &[], &["opt_key"]);
    let m = v.value_model("opt_key").await.unwrap();
    assert_eq!(
        (m.set, m.usd_per_unit, m.unit.as_deref()),
        (true, Some(1000.0), Some("ms"))
    );
    let before = v
        .overview(OverviewRange::All)
        .await
        .unwrap()
        .story
        .delivered
        .usd_per_year;
    close(before, 10_000.0);

    let set = v
        .set_value_model(
            "opt_key",
            SetValueModelInput {
                usd_per_unit: 2500.0,
                period: Some(ValuePeriod::Month),
                basis: Some(ValueBasis::Measured),
                unit: Some("ms".into()),
                note: Some("ms saved x checkout volume".into()),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        set.summary,
        "$2,500 per ms a month \u{b7} measured \u{b7} ms saved x checkout volume"
    );
    assert_eq!(v.value_model("opt_key").await.unwrap(), set);
    // the same 10 ms, now $2,500 a month = $300,000 a year
    close(
        v.overview(OverviewRange::All)
            .await
            .unwrap()
            .story
            .delivered
            .usd_per_year,
        300_000.0,
    );

    // not a model: zero, negative, too long
    for bad in [0.0, -3.0] {
        let e = v
            .set_value_model(
                "opt_key",
                SetValueModelInput {
                    usd_per_unit: bad,
                    period: None,
                    basis: None,
                    unit: None,
                    note: None,
                },
            )
            .await
            .unwrap_err();
        assert!(e.to_string().contains("greater than 0"), "{e}");
    }
    let e = v
        .set_value_model(
            "opt_key",
            SetValueModelInput {
                usd_per_unit: 1.0,
                period: None,
                basis: None,
                unit: None,
                note: Some("x".repeat(201)),
            },
        )
        .await
        .unwrap_err();
    assert!(e.to_string().contains("note is at most 200"), "{e}");

    let cleared = v.clear_value_model("opt_key").await.unwrap();
    assert!(!cleared.set && cleared.summary == "Not set" && cleared.note.is_none());
    let o = v.overview(OverviewRange::All).await.unwrap();
    assert_eq!(
        o.rows
            .iter()
            .find(|r| r.id == "opt_key")
            .unwrap()
            .value_usd_per_year,
        None
    );
    assert!(v.value_model("nope").await.is_err());
}

#[tokio::test]
async fn the_schedule_round_trips() {
    let sim = mixed().await;
    let v = views(&sim, &[], &[]);
    let s = v.schedule("opt_ready").await.unwrap();
    assert_eq!(
        (s.schedule, s.summary.as_str(), s.runs_next_30_days),
        (None, "Not scheduled", None)
    );
    let s = v
        .set_schedule(
            "opt_ready",
            SetScheduleInput {
                schedule: Some(ScheduleKind::Weekdays),
            },
        )
        .await
        .unwrap();
    assert_eq!(
        (s.summary.as_str(), s.runs_next_30_days),
        ("Weekdays at night", Some(21))
    );
    assert_eq!(v.schedule("opt_ready").await.unwrap(), s);
    let s = v
        .set_schedule(
            "opt_ready",
            SetScheduleInput {
                schedule: Some(ScheduleKind::OnPush),
            },
        )
        .await
        .unwrap();
    assert_eq!(s.runs_next_30_days, None);
    let s = v
        .set_schedule("opt_ready", SetScheduleInput { schedule: None })
        .await
        .unwrap();
    assert_eq!(s.schedule, None);
}

#[tokio::test]
async fn recording_a_merge_turns_pending_value_into_delivered() {
    let sim = sim(true, true).await;
    let before = sim.overview(OverviewRange::Days90).await.unwrap().story;
    // the seeded open pull request is ingest's; it has no merge recorded yet
    let pr = sim
        .mark_pull_request_merged("opt_demo_ingest")
        .await
        .unwrap();
    assert!(pr.url.ends_with("/pull/311"));
    let after = sim.overview(OverviewRange::Days90).await.unwrap().story;
    close(after.pending.usd_per_year, 0.0);
    close(
        after.delivered.usd_per_year,
        before.delivered.usd_per_year + before.pending.usd_per_year,
    );
    assert!(after.return_x.unwrap() > before.return_x.unwrap());
    // without a pull request there is nothing to mark
    assert!(
        sim.mark_pull_request_merged("opt_demo_search")
            .await
            .is_err()
    );
}
