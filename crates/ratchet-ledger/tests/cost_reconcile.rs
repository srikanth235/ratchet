//! Port of `packages/core/src/cost/reconcile.test.ts`, running `reconcile_spend` against the SQLite ledger.

mod common;

use std::collections::BTreeMap;

use common::*;
use ratchet_core::cost::{
    COST_EXPLORER_LAG_MS, ReconcileError, ReconcileSpendOptions, reconcile_spend,
};
use ratchet_core::enums::{Direction, OptimizationStatus, SpendCategory};
use ratchet_core::ids::OptimizationId;
use ratchet_core::ports::mock::MockCostReconciler;
use ratchet_core::ports::{
    CostActuals, CostActualsQuery, CostReconcileStatus, CostReconciler, PortError, PortResult,
};
use ratchet_ledger::{CreateOptimizationInput, Ledger, RecordSpendInput};
use sqlx::Row;

const HOUR: i64 = 3_600_000;
const T0: i64 = 1_700_000_000_000;

async fn setup() -> (Ledger, OptimizationId) {
    let l = Ledger::open_memory(ratchet_ledger::LedgerOptions {
        clock: Some(TickClock::new(T0)),
        max_connections: None,
    })
    .await
    .unwrap();
    let opt = l
        .create_optimization(CreateOptimizationInput::new(
            "acme/search",
            "main",
            "p95",
            Direction::Minimize,
            0.03,
            100.0,
            3600,
            "version: 0\n",
        ))
        .await
        .unwrap();
    l.set_optimization_status(&opt.id, OptimizationStatus::Running, Some(T0))
        .await
        .unwrap();
    for (category, usd, source, at) in [
        (SpendCategory::AgentRunner, 1.0, "ec2:i-1", T0 + 1),
        (SpendCategory::Experiment, 2.0, "ec2:i-2", T0 + 2),
        (SpendCategory::Tokens, 5.0, "agent", T0 + 3),
    ] {
        let mut s = RecordSpendInput::new(opt.id.clone(), category, usd, source);
        s.at = Some(at);
        l.record_spend(s).await.unwrap();
    }
    (l, opt.id)
}

fn final_actuals(by: &[(SpendCategory, f64)]) -> CostActuals {
    CostActuals {
        by_category: by.iter().copied().collect(),
        groups: by.len() as u32,
        complete: true,
        note: None,
    }
}

#[derive(Debug, PartialEq)]
struct Correction {
    category: String,
    usd: f64,
    estimated: i64,
    source: String,
    agent_id: Option<String>,
}

async fn corrections(l: &Ledger) -> Vec<Correction> {
    sqlx::query("select category, usd, estimated, source, agent_id from spend_events where source = 'cost-explorer' order by id")
        .fetch_all(l.pool())
        .await
        .unwrap()
        .iter()
        .map(|r| Correction {
            category: r.get("category"),
            usd: r.get("usd"),
            estimated: r.get("estimated"),
            source: r.get("source"),
            agent_id: r.get("agent_id"),
        })
        .collect()
}

const END_AT: i64 = T0 + 2 * HOUR;
const READY: i64 = END_AT + COST_EXPLORER_LAG_MS + 1;

/// A reconciler whose answer can change between calls.
struct Scripted(std::sync::Mutex<CostActuals>);

#[async_trait::async_trait]
impl CostReconciler for Scripted {
    async fn actuals(&self, _q: &CostActualsQuery) -> PortResult<CostActuals> {
        Ok(self.0.lock().unwrap().clone())
    }
}

// "writes positive and negative corrections as estimated=false / source=cost-explorer, leaving estimates and tokens alone"
#[tokio::test]
async fn writes_positive_and_negative_corrections_leaving_estimates_and_tokens_alone() {
    let (l, id) = setup().await;
    l.set_optimization_status(&id, OptimizationStatus::Stopped, Some(END_AT))
        .await
        .unwrap();
    let r = MockCostReconciler::complete(&[
        (SpendCategory::AgentRunner, 1.5),
        (SpendCategory::Experiment, 1.25),
        (SpendCategory::BenchRunner, 0.3),
    ]);
    let rep = reconcile_spend(&l, &r, &id, ReconcileSpendOptions::at(READY))
        .await
        .unwrap();
    assert_eq!(rep.status, CostReconcileStatus::Reconciled);
    assert_eq!(
        r.calls(),
        [CostActualsQuery {
            optimization_id: id.clone(),
            start_ms: T0,
            end_ms: END_AT
        }]
    );
    let rows = corrections(&l).await;
    let rounded: Vec<(String, f64)> = rows
        .iter()
        .map(|x| (x.category.clone(), (x.usd * 1e9).round() / 1e9))
        .collect();
    assert_eq!(
        rounded,
        [
            ("agent_runner".to_owned(), 0.5),
            ("experiment".to_owned(), -0.75),
            ("bench_runner".to_owned(), 0.3)
        ] // storage_egress: billed 0, ledger 0 -> nothing
    );
    assert!(
        rows.iter()
            .all(|x| x.estimated == 0 && x.source == "cost-explorer" && x.agent_id.is_none())
    );
    let b = l.spend_breakdown(&id).await.unwrap();
    let c = b.by_category;
    assert_eq!(
        (c.agent_runner, c.experiment, c.bench_runner, c.tokens),
        (1.5, 1.25, 0.3, 5.0)
    );
    assert_eq!(b.estimated, 8.0);
    assert!((rep.written_usd - 0.05).abs() < 1e-9);
}

// "is idempotent: re-running with the same data writes nothing; later data writes only the remainder"
#[tokio::test]
async fn is_idempotent_and_later_data_writes_only_the_remainder() {
    let (l, id) = setup().await;
    l.set_optimization_status(&id, OptimizationStatus::Stopped, Some(END_AT))
        .await
        .unwrap();
    let r = Scripted(std::sync::Mutex::new(final_actuals(&[
        (SpendCategory::AgentRunner, 1.5),
        (SpendCategory::Experiment, 2.0),
    ])));
    assert_eq!(
        reconcile_spend(&l, &r, &id, ReconcileSpendOptions::at(READY))
            .await
            .unwrap()
            .status,
        CostReconcileStatus::Reconciled
    );
    let n = corrections(&l).await.len();
    for i in 0..3 {
        let again = reconcile_spend(&l, &r, &id, ReconcileSpendOptions::at(READY + i))
            .await
            .unwrap();
        assert_eq!(again.status, CostReconcileStatus::Unchanged);
        assert_eq!(again.written_usd, 0.0);
    }
    assert_eq!(corrections(&l).await.len(), n);
    *r.0.lock().unwrap() = final_actuals(&[
        (SpendCategory::AgentRunner, 1.75),
        (SpendCategory::Experiment, 2.0),
    ]); // billing finalised a little higher
    reconcile_spend(&l, &r, &id, ReconcileSpendOptions::at(READY + 10))
        .await
        .unwrap();
    let rows = corrections(&l).await;
    assert_eq!(rows.len(), n + 1);
    assert!((rows.last().unwrap().usd - 0.25).abs() < 1e-9);
    assert!(
        (l.spend_breakdown(&id)
            .await
            .unwrap()
            .by_category
            .agent_runner
            - 1.75)
            .abs()
            < 1e-9
    );
}

// "is pending (no rows, no billing query) while the optimization runs or inside the billing lag"
#[tokio::test]
async fn is_pending_while_the_optimization_runs_or_inside_the_billing_lag() {
    let (l, id) = setup().await;
    let r = MockCostReconciler::complete(&[(SpendCategory::AgentRunner, 9.0)]);
    let running = reconcile_spend(&l, &r, &id, ReconcileSpendOptions::at(READY))
        .await
        .unwrap();
    assert_eq!(
        (running.status, running.written_usd),
        (CostReconcileStatus::Pending, 0.0)
    );
    assert!(running.reason.as_deref().unwrap().contains("running"));
    l.set_optimization_status(&id, OptimizationStatus::Stopped, Some(END_AT))
        .await
        .unwrap();
    let lagging = reconcile_spend(&l, &r, &id, ReconcileSpendOptions::at(END_AT + 3 * HOUR))
        .await
        .unwrap();
    assert_eq!(lagging.status, CostReconcileStatus::Pending);
    let reason = lagging.reason.unwrap();
    assert!(
        reason.contains("lags up to 24 h") && reason.contains("21 h"),
        "{reason}"
    );
    assert_eq!(r.calls().len(), 0);
    assert_eq!(corrections(&l).await, vec![]);
}

// "is pending, writing nothing, when billing is flagged incomplete or returns no data (tags not activated)"
#[tokio::test]
async fn is_pending_when_billing_is_flagged_incomplete_or_returns_no_data() {
    let (l, id) = setup().await;
    l.set_optimization_status(&id, OptimizationStatus::Stopped, Some(END_AT))
        .await
        .unwrap();
    let incomplete = MockCostReconciler::new(CostActuals {
        by_category: [(SpendCategory::AgentRunner, 0.1)].into(),
        groups: 1,
        complete: false,
        note: Some("still estimated".into()),
    });
    let rep = reconcile_spend(&l, &incomplete, &id, ReconcileSpendOptions::at(READY))
        .await
        .unwrap();
    assert_eq!(
        (rep.status, rep.reason.as_deref()),
        (CostReconcileStatus::Pending, Some("still estimated"))
    );
    let line = rep
        .lines
        .iter()
        .find(|l| l.category == SpendCategory::AgentRunner)
        .unwrap();
    assert_eq!(
        (line.ledger_usd, line.actual_usd, line.written),
        (1.0, 0.1, false)
    );

    let empty = MockCostReconciler::new(CostActuals {
        by_category: BTreeMap::new(),
        groups: 0,
        complete: true,
        note: None,
    });
    let rep = reconcile_spend(&l, &empty, &id, ReconcileSpendOptions::at(READY))
        .await
        .unwrap();
    assert_eq!(rep.status, CostReconcileStatus::Pending);
    assert!(rep.reason.unwrap().contains("no cost data"));
    assert_eq!(corrections(&l).await, vec![]);
    assert_eq!(
        l.spend_breakdown(&id)
            .await
            .unwrap()
            .by_category
            .agent_runner,
        1.0
    ); // not zeroed out
}

// "empty billing is fine when the ledger holds no infrastructure estimate"
#[tokio::test]
async fn empty_billing_is_fine_when_the_ledger_holds_no_infrastructure_estimate() {
    let (l, _) = setup().await;
    let o = l
        .create_optimization(CreateOptimizationInput::new(
            "a/b",
            "main",
            "p95",
            Direction::Minimize,
            0.03,
            1.0,
            60,
            "v",
        ))
        .await
        .unwrap();
    l.set_optimization_status(&o.id, OptimizationStatus::Stopped, Some(END_AT))
        .await
        .unwrap();
    let empty = MockCostReconciler::new(CostActuals {
        by_category: BTreeMap::new(),
        groups: 0,
        complete: true,
        note: None,
    });
    let rep = reconcile_spend(&l, &empty, &o.id, ReconcileSpendOptions::at(READY))
        .await
        .unwrap();
    assert_eq!(rep.status, CostReconcileStatus::Unchanged);
}

// "honours lagMs and propagates billing errors"
#[tokio::test]
async fn honours_lag_ms_and_propagates_billing_errors() {
    let (l, id) = setup().await;
    l.set_optimization_status(&id, OptimizationStatus::Stopped, Some(END_AT))
        .await
        .unwrap();
    let ok = MockCostReconciler::complete(&[
        (SpendCategory::AgentRunner, 1.0),
        (SpendCategory::Experiment, 2.0),
    ]);
    let opts = ReconcileSpendOptions {
        now: END_AT + 1,
        lag_ms: Some(0),
        epsilon_usd: None,
    };
    assert_eq!(
        reconcile_spend(&l, &ok, &id, opts).await.unwrap().status,
        CostReconcileStatus::Unchanged
    );
    let boom = MockCostReconciler::failing("AccessDenied");
    let e = reconcile_spend(&l, &boom, &id, ReconcileSpendOptions::at(READY))
        .await
        .unwrap_err();
    assert_eq!(
        e,
        ReconcileError::Billing(PortError::failed("AccessDenied"))
    );
    assert_eq!(e.to_string(), "AccessDenied");
    let missing = reconcile_spend(&l, &boom, &"nope".into(), ReconcileSpendOptions::at(READY))
        .await
        .unwrap_err();
    assert!(missing.to_string().contains("not found"));
}

// ---------------------------------------------------------------- beyond the TypeScript cases

/// The correction step runs in one write transaction: two concurrent reconciles never both correct one gap.
#[tokio::test]
async fn concurrent_reconciles_correct_a_gap_only_once() {
    let (l, id) = setup().await;
    l.set_optimization_status(&id, OptimizationStatus::Stopped, Some(END_AT))
        .await
        .unwrap();
    let r = std::sync::Arc::new(MockCostReconciler::complete(&[
        (SpendCategory::AgentRunner, 4.0),
        (SpendCategory::Experiment, 2.0),
    ]));
    let mut tasks = Vec::new();
    for i in 0..6 {
        let (l, r, id) = (l.clone(), r.clone(), id.clone());
        tasks.push(tokio::spawn(async move {
            reconcile_spend(&l, &*r, &id, ReconcileSpendOptions::at(READY + i)).await
        }));
    }
    for t in tasks {
        t.await.unwrap().unwrap();
    }
    let rows = corrections(&l).await;
    assert_eq!(rows.len(), 1, "{rows:?}");
    assert!((rows[0].usd - 3.0).abs() < 1e-9);
    assert!(
        (l.spend_breakdown(&id)
            .await
            .unwrap()
            .by_category
            .agent_runner
            - 4.0)
            .abs()
            < 1e-9
    );
}
