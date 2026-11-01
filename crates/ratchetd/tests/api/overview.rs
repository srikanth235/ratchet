//! The Overview (ported from `packages/desktop/src/main/overview.test.ts`): spent, value locked in, the staircase,
//! what needs the user and the table, computed from the seeded demo ledger at a fixed time.

use std::sync::Arc;

use ratchet_core::enums::{OptimizationStatus, ValueBasis};
use ratchet_core::ids::OptimizationId;
use ratchet_core::ports::Clock;
use ratchet_core::ports::mock::ManualClock;
use ratchetd::api::*;
use ratchetd::sim::{SimOptions, SimService};
use ratchetd::views::{LedgerViews, PrUrlLookup};

/// 2026-10-30T12:00:00Z, the `NOW` of the TS test.
const NOW: i64 = 1_793_361_600_000;
const DAY: i64 = 86_400_000;
const RANGES: [OverviewRange; 3] = [
    OverviewRange::Days30,
    OverviewRange::Days90,
    OverviewRange::All,
];

fn no_pr() -> PrUrlLookup {
    Arc::new(|_| None)
}

fn close(a: f64, b: f64, eps: f64) {
    assert!((a - b).abs() < eps, "{a} is not within {eps} of {b}");
}

/// The seeded demo ledger at `NOW`, nothing running by itself.
async fn seeded() -> (SimService, LedgerViews) {
    let sim = sim_with(true).await;
    let views = views_of(&sim, no_pr());
    (sim, views)
}

async fn sim_with(seed_demo: bool) -> SimService {
    let clock: Arc<dyn Clock> = Arc::new(ManualClock::new(NOW));
    SimService::new(SimOptions {
        seed_demo,
        auto_run: false,
        resume_runs: false,
        github_flow_ms: 0,
        clock: Some(clock),
        ..SimOptions::default()
    })
    .await
    .unwrap()
}

fn views_of(sim: &SimService, pr: PrUrlLookup) -> LedgerViews {
    LedgerViews::new(sim.ledger().clone(), Arc::new(ManualClock::new(NOW)), pr)
}

fn views_at(sim: &SimService, now: i64) -> LedgerViews {
    LedgerViews::new(
        sim.ledger().clone(),
        Arc::new(ManualClock::new(now)),
        no_pr(),
    )
}

async fn sql(sim: &SimService, q: &'static str) {
    sqlx::query(q).execute(sim.ledger().pool()).await.unwrap();
}

async fn total_spend(sim: &SimService, id: &str) -> f64 {
    sim.ledger()
        .list_optimizations(None)
        .await
        .unwrap()
        .into_iter()
        .find(|o| o.id.as_str() == id)
        .unwrap()
        .spend_usd
}

fn ids(o: &OverviewVm) -> Vec<&str> {
    o.rows.iter().map(|r| r.id.as_str()).collect()
}

fn row<'a>(o: &'a OverviewVm, id: &str) -> &'a OverviewRow {
    o.rows.iter().find(|r| r.id == id).unwrap()
}

// ---- the numbers -----------------------------------------------------------------------------------------------

/// TS: `buildOverview: the numbers` / sums spend and value over the range, and relates them (return per $1, payback in days)
#[tokio::test]
async fn sums_spend_and_value_over_the_range_and_relates_them() {
    let (sim, v) = seeded().await;
    let o = v.overview(OverviewRange::Days30).await.unwrap();
    // search + ingest finished or run inside 30 days; the edge worker is 41 days old
    close(
        o.spent.usd,
        total_spend(&sim, "opt_demo_search").await + total_spend(&sim, "opt_demo_ingest").await,
        1e-3,
    );
    // search: 182 -> 131 ms at $1,200 per ms; ingest: 41.2 -> 58.9 k/s at $400 per k/s
    let value = o.value.as_ref().unwrap();
    close(value.usd_per_year, 51.0 * 1200.0 + 17.7 * 400.0, 1e-3);
    assert_eq!(value.wins, 7);
    assert_eq!(value.basis, ValueBasis::Estimated);
    let ret = o.return_on_spend.as_ref().unwrap();
    close(ret.ratio, value.usd_per_year / o.spent.usd, 1e-6);
    close(
        ret.payback_days,
        (o.spent.usd / value.usd_per_year) * 365.0,
        1e-6,
    );
    assert_eq!(o.spent.budget_usd, 120.0 + 100.0); // search and ingest (a draft has no budget to spend yet)
    assert_eq!(o.spent.optimizations, 2);
    assert!(!o.empty);
}

/// TS: `buildOverview: the numbers` / filters by range: 90 days adds the older optimization, all time starts at the first activity
#[tokio::test]
async fn filters_by_range_90_days_adds_the_older_optimization_all_time_starts_at_the_first_activity()
 {
    let (sim, v) = seeded().await;
    let d30 = v.overview(OverviewRange::Days30).await.unwrap();
    let d90 = v.overview(OverviewRange::Days90).await.unwrap();
    let all = v.overview(OverviewRange::All).await.unwrap();
    assert_eq!(
        ids(&d30),
        ["opt_demo_search", "opt_demo_ingest", "opt_demo_web"]
    );
    assert_eq!(
        ids(&d90),
        [
            "opt_demo_search",
            "opt_demo_ingest",
            "opt_demo_web",
            "opt_demo_edge"
        ]
    );
    assert_eq!(d90.value.as_ref().unwrap().wins, 8);
    close(
        d90.value.as_ref().unwrap().usd_per_year,
        d30.value.as_ref().unwrap().usd_per_year + 0.79 * 3000.0,
        1e-3,
    );
    assert_eq!(d90.from, NOW - 90 * DAY);
    // the edge optimization started first (its baseline took 3 minutes)
    assert_eq!(all.from, NOW - 41 * DAY + 3 * 60_000);
    close(all.spent.usd, d90.spent.usd, 1e-6);
    assert_eq!(d30.range, OverviewRange::Days30);
    // a window with nothing in it
    let quiet = views_at(&sim, NOW + 400 * DAY)
        .overview(OverviewRange::Days30)
        .await
        .unwrap();
    assert_eq!(quiet.spent.usd, 0.0);
    let qv = quiet.value.as_ref().unwrap();
    assert_eq!(qv.usd_per_year, 0.0);
    assert_eq!(qv.wins, 0);
    assert!(quiet.return_on_spend.is_none());
    assert!(quiet.wins.is_empty());
}

/// TS: `buildOverview: the numbers` / the table adds up to the hero: per-row spend and value sum to the totals
#[tokio::test]
async fn the_table_adds_up_to_the_hero() {
    let (_sim, v) = seeded().await;
    for range in RANGES {
        let o = v.overview(range).await.unwrap();
        close(
            o.rows.iter().map(|r| r.spent_usd).sum::<f64>(),
            o.spent.usd,
            1e-6,
        );
        close(
            o.rows
                .iter()
                .map(|r| r.value_usd_per_year.unwrap_or(0.0))
                .sum::<f64>(),
            o.value.as_ref().unwrap().usd_per_year,
            1e-6,
        );
    }
}

/// TS: `buildOverview: the numbers` / the staircase is the cumulative value of the wins in the range, oldest first, naming each optimization
#[tokio::test]
async fn the_staircase_is_the_cumulative_value_of_the_wins_in_the_range() {
    let (_sim, v) = seeded().await;
    let o = v.overview(OverviewRange::Days90).await.unwrap();
    assert_eq!(o.wins.len(), 8);
    assert!(
        o.wins
            .windows(2)
            .all(|w| w[1].at >= w[0].at
                && w[1].cumulative_usd_per_year > w[0].cumulative_usd_per_year)
    );
    close(
        o.wins.last().unwrap().cumulative_usd_per_year,
        o.value.as_ref().unwrap().usd_per_year,
        1e-6,
    );
    assert_eq!(o.wins[0].optimization, "Edge worker cold start");
    close(o.wins[0].usd_per_year, 0.79 * 3000.0, 1e-6);
    let names: std::collections::BTreeSet<&str> =
        o.wins.iter().map(|w| w.optimization.as_str()).collect();
    assert_eq!(
        names,
        std::collections::BTreeSet::from([
            "Edge worker cold start",
            "Ingest throughput",
            "search-api p95 latency"
        ])
    );
}

/// TS: `buildOverview: the numbers` / a row says what happened: gain, baseline -> best, wins, status word, time left while running
#[tokio::test]
async fn a_row_says_what_happened() {
    let (_sim, v) = seeded().await;
    let o = v.overview(OverviewRange::Days30).await.unwrap();
    let search = row(&o, "opt_demo_search");
    assert_eq!(search.status_label, "Running");
    assert_eq!(search.tone, StatusTone::Run);
    assert!(search.live);
    assert_eq!(search.unit, "ms");
    assert_eq!(search.direction, OverviewDirection::Lower);
    assert_eq!(search.baseline, Some(182.0));
    assert_eq!(search.best, Some(131.0));
    assert_eq!(search.verified_wins, 5);
    assert_eq!(search.budget_usd, 120.0);
    close(search.gain_pct.unwrap(), 28.0, 0.5);
    // time limit minus the 221 minutes since it went live
    assert_eq!(search.remaining_s, Some(8 * 3600 - 221 * 60));
    close(search.value_usd_per_year.unwrap(), 51.0 * 1200.0, 1e-3);
    assert_eq!(o.rows[0].id, "opt_demo_search"); // live first
    let ingest = row(&o, "opt_demo_ingest");
    assert_eq!(ingest.status_label, "Waiting on you");
    assert_eq!(ingest.tone, StatusTone::Wait);
    assert_eq!(ingest.remaining_s, None);
    assert!(!ingest.live);
    let web = row(&o, "opt_demo_web");
    assert_eq!(web.status_label, "Draft");
    assert_eq!(web.tone, StatusTone::Off);
    assert_eq!(web.baseline, None);
    assert_eq!(web.gain_pct, None);
    assert_eq!(web.value_usd_per_year, None);
}

// ---- no value model, no optimizations --------------------------------------------------------------------------

/// TS: `buildOverview: no value model, no optimizations` / shows no value and no return when nothing has a value model, but still adds up the spend
#[tokio::test]
async fn shows_no_value_and_no_return_when_nothing_has_a_value_model() {
    let (sim, v) = seeded().await;
    sql(&sim, "UPDATE optimizations SET value_usd_per_unit = NULL").await;
    let o = v.overview(OverviewRange::Days90).await.unwrap();
    assert!(o.value.is_none());
    assert!(o.return_on_spend.is_none());
    assert!(o.wins.is_empty());
    assert!(o.spent.usd > 100.0);
    assert!(o.rows.iter().all(|r| r.value_usd_per_year.is_none()));
    assert_eq!(row(&o, "opt_demo_search").verified_wins, 5);
}

/// TS: `buildOverview: no value model, no optimizations` / an empty ledger is all zeros and empty lists, over any range
#[tokio::test]
async fn an_empty_ledger_is_all_zeros_and_empty_lists_over_any_range() {
    let sim = sim_with(false).await;
    let v = views_of(&sim, no_pr());
    for range in RANGES {
        let o = v.overview(range).await.unwrap();
        assert_eq!(o.range, range);
        assert!(o.empty);
        assert_eq!(o.spent.usd, 0.0);
        assert_eq!(o.spent.budget_usd, 0.0);
        assert_eq!(o.spent.optimizations, 0);
        assert!(o.value.is_none());
        assert!(o.return_on_spend.is_none());
        assert!(o.wins.is_empty());
        assert!(o.needs_you.is_empty());
        assert!(o.rows.is_empty());
        assert!(o.from < o.to);
    }
}

/// TS: `buildOverview: no value model, no optimizations` / measured only when every contributing model is measured
#[tokio::test]
async fn measured_only_when_every_contributing_model_is_measured() {
    let (sim, v) = seeded().await;
    sql(&sim, "UPDATE optimizations SET value_basis = 'measured'").await;
    assert_eq!(
        v.overview(OverviewRange::Days90)
            .await
            .unwrap()
            .value
            .unwrap()
            .basis,
        ValueBasis::Measured
    );
    sql(
        &sim,
        "UPDATE optimizations SET value_basis = 'estimated' WHERE id = 'opt_demo_ingest'",
    )
    .await;
    assert_eq!(
        v.overview(OverviewRange::Days90)
            .await
            .unwrap()
            .value
            .unwrap()
            .basis,
        ValueBasis::Estimated
    );
}

// ---- needs you -------------------------------------------------------------------------------------------------

/// TS: `buildOverview: needs you` / lists finished runs with verified wins and no pull request, whatever the range; "Waiting on you" in the table
#[tokio::test]
async fn lists_finished_runs_with_verified_wins_and_no_pull_request() {
    let (_sim, v) = seeded().await;
    let o = v.overview(OverviewRange::Days30).await.unwrap();
    let got: Vec<(OverviewNeedKind, &str, &str, &str)> = o
        .needs_you
        .iter()
        .map(|n| {
            (
                n.kind,
                n.optimization_id.as_str(),
                n.title.as_str(),
                n.sub.as_str(),
            )
        })
        .collect();
    assert_eq!(
        got,
        [
            (
                OverviewNeedKind::Ship,
                "opt_demo_ingest",
                "Open a pull request for 2 verified wins",
                "Ingest throughput \u{b7} +43%"
            ),
            (
                OverviewNeedKind::Ship,
                "opt_demo_edge",
                "Open a pull request for 1 verified win",
                "Edge worker cold start \u{b7} \u{2212}33%"
            ),
        ]
    );
    assert!(o.needs_you.iter().all(|n| n.action == "Review"));
}

/// TS: `buildOverview: needs you` / a pull request takes it off the list and the status reads "PR open"
#[tokio::test]
async fn a_pull_request_takes_it_off_the_list_and_the_status_reads_pr_open() {
    let (sim, _) = seeded().await;
    let with_pr: PrUrlLookup = Arc::new(|id| {
        (id == "opt_demo_ingest").then(|| "https://github.com/acme/ingest/pull/7".to_owned())
    });
    let o = views_of(&sim, with_pr)
        .overview(OverviewRange::Days30)
        .await
        .unwrap();
    let need_ids: Vec<&str> = o
        .needs_you
        .iter()
        .map(|n| n.optimization_id.as_str())
        .collect();
    assert_eq!(need_ids, ["opt_demo_edge"]);
    let ingest = row(&o, "opt_demo_ingest");
    assert_eq!(ingest.status, UiStatus::PrOpen);
    assert_eq!(ingest.status_label, "PR open");
    assert_eq!(ingest.tone, StatusTone::Ok);
}

/// TS: `buildOverview: needs you` / a recent failed run asks for a look; an old one stops asking
#[tokio::test]
async fn a_recent_failed_run_asks_for_a_look_an_old_one_stops_asking() {
    let (sim, v) = seeded().await;
    let ingest = OptimizationId::new("opt_demo_ingest");
    sim.ledger()
        .set_optimization_status(&ingest, OptimizationStatus::Failed, Some(NOW - 2 * DAY))
        .await
        .unwrap();
    let o = v.overview(OverviewRange::Days30).await.unwrap();
    let failed = o
        .needs_you
        .iter()
        .find(|n| n.kind == OverviewNeedKind::Failed)
        .unwrap();
    assert_eq!(failed.optimization_id, "opt_demo_ingest");
    assert_eq!(failed.title, "A run failed");
    assert_eq!(failed.sub, "Ingest throughput");
    assert_eq!(failed.action, "View");
    let r = row(&o, "opt_demo_ingest");
    assert_eq!(r.status_label, "Failed");
    assert_eq!(r.tone, StatusTone::Bad);
    sim.ledger()
        .set_optimization_status(&ingest, OptimizationStatus::Failed, Some(NOW - 20 * DAY))
        .await
        .unwrap();
    let o = v.overview(OverviewRange::Days30).await.unwrap();
    assert!(
        !o.needs_you
            .iter()
            .any(|n| n.kind == OverviewNeedKind::Failed)
    );
}

/// TS: `buildOverview: needs you` / a finished run without a win reads "No win" and is not waiting on anyone
#[tokio::test]
async fn a_finished_run_without_a_win_reads_no_win_and_is_not_waiting_on_anyone() {
    let (sim, v) = seeded().await;
    sim.ledger()
        .set_optimization_status(
            &OptimizationId::new("opt_demo_web"),
            OptimizationStatus::Completed,
            Some(NOW - DAY),
        )
        .await
        .unwrap();
    let o = v.overview(OverviewRange::Days30).await.unwrap();
    assert!(
        !o.needs_you
            .iter()
            .any(|n| n.optimization_id == "opt_demo_web")
    );
    let web = row(&o, "opt_demo_web");
    assert_eq!(web.status_label, "No win");
    assert_eq!(web.tone, StatusTone::Off);
    assert_eq!(web.verified_wins, 0);
}
