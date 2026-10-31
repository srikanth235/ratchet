//! `packages/engine/src/reconcile.test.ts`

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ratchet_core::enums::SpendCategory;
use ratchet_core::ports::{
    CostActuals, CostActualsQuery, CostReconcileStatus, CostReconciler, EngineEvent, PortError,
    PortResult,
};
use ratchet_engine::{EngineErrorCode, EngineSettings};
use sqlx::Row;

use crate::common::*;

const HOUR: i64 = 3_600_000;

#[derive(Default)]
struct Billing {
    calls: Mutex<Vec<CostActualsQuery>>,
}

#[async_trait]
impl CostReconciler for Billing {
    async fn actuals(&self, q: &CostActualsQuery) -> PortResult<CostActuals> {
        self.calls.lock().unwrap().push(q.clone());
        Ok(CostActuals {
            by_category: BTreeMap::from([
                (SpendCategory::AgentRunner, 0.5),
                (SpendCategory::Experiment, 0.001),
                (SpendCategory::BenchRunner, 0.25),
            ]),
            groups: 3,
            complete: true,
            note: None,
        })
    }
}

struct Failing;

#[async_trait]
impl CostReconciler for Failing {
    async fn actuals(&self, _q: &CostActualsQuery) -> PortResult<CostActuals> {
        Err(PortError::failed("ce down"))
    }
}

/// (category, usd, estimated, source) of the cost-explorer rows.
async fn spend_rows(h: &Harness, id: &str) -> Vec<(String, f64, i64, String)> {
    sqlx::query("select category, usd, estimated, source from spend_events where optimization_id = ? and source = ? order by id")
        .bind(id)
        .bind("cost-explorer")
        .fetch_all(h.ledger.pool())
        .await
        .unwrap()
        .iter()
        .map(|r| (r.get("category"), r.get("usd"), r.get("estimated"), r.get("source")))
        .collect()
}

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-9, "{a} != {b}");
}

#[tokio::test]
async fn is_unavailable_without_a_cost_reconciler_and_teardown_then_records_no_reconcile() {
    let h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    let e = h.engine.reconcile_costs(&b.id).await.unwrap_err();
    assert_eq!(e.code, EngineErrorCode::Unavailable);
    h.engine.teardown(&b.id).await.unwrap();
    assert!(
        h.engine
            .teardown_report(&b.id)
            .await
            .unwrap()
            .unwrap()
            .cost_reconcile
            .is_none()
    );
}

#[tokio::test]
async fn teardown_reconciles_best_effort_pending_inside_the_billing_lag_nothing_written_no_billing_query()
 {
    let costs = Arc::new(Billing::default());
    let h = Harness::new(HarnessOptions {
        costs: Some(costs.clone()),
        ..Default::default()
    })
    .await;
    let b = begun(&h).await;
    h.compute.launch_experiment(&b.id, &b.agent(Codex));
    h.cycle(2).await;
    h.engine.teardown(&b.id).await.unwrap();
    assert_eq!(h.status(&b.id).await, OptimizationStatus::Completed);
    let cr = h
        .engine
        .teardown_report(&b.id)
        .await
        .unwrap()
        .unwrap()
        .cost_reconcile
        .unwrap();
    assert_eq!(cr.status, "pending");
    assert_eq!(cr.written_usd, 0.0);
    assert!(spend_rows(&h, &b.id).await.is_empty());
    assert_eq!(costs.calls.lock().unwrap().len(), 0);
}

#[tokio::test]
async fn writes_correction_rows_once_billing_is_final_and_a_second_run_changes_nothing() {
    let costs = Arc::new(Billing::default());
    let mut h = Harness::new(HarnessOptions {
        costs: Some(costs.clone()),
        ..Default::default()
    })
    .await;
    let b = begun(&h).await;
    h.compute.launch_experiment(&b.id, &b.agent(Codex));
    h.cycle(3).await;
    h.engine.teardown(&b.id).await.unwrap();
    let before = h.ledger.spend_breakdown(&oid(&b.id)).await.unwrap();
    assert_eq!(before.actual, 0.0);

    h.clock.advance(25 * HOUR);
    let r1 = h.engine.reconcile_costs(&b.id).await.unwrap();
    assert_eq!(r1.status, CostReconcileStatus::Reconciled);
    let rows = spend_rows(&h, &b.id).await;
    assert!(!rows.is_empty());
    assert!(rows.iter().all(|r| r.2 == 0 && r.3 == "cost-explorer"));
    let after = h.ledger.spend_breakdown(&oid(&b.id)).await.unwrap();
    close(after.by_category.agent_runner, 0.5);
    close(after.by_category.experiment, 0.001);
    close(after.by_category.bench_runner, 0.25);
    close(after.estimated, before.estimated); // estimates are never rewritten
    assert_eq!(
        h.engine
            .cost_reconcile_report(&b.id)
            .await
            .unwrap()
            .unwrap()
            .status,
        CostReconcileStatus::Reconciled
    );
    assert!(
        h.events()
            .iter()
            .any(|e| matches!(e, EngineEvent::Notice { text, .. } if text.contains("reconciled")))
    );

    let r2 = h.engine.reconcile_costs(&b.id).await.unwrap();
    assert_eq!(r2.status, CostReconcileStatus::Unchanged);
    assert_eq!(spend_rows(&h, &b.id).await.len(), rows.len());
    close(
        h.ledger.total_spend(&oid(&b.id)).await.unwrap(),
        after.total,
    );
}

#[tokio::test]
async fn a_failing_reconciler_never_fails_teardown() {
    let h = Harness::new(HarnessOptions {
        costs: Some(Arc::new(Failing)),
        settings: EngineSettings {
            cost_lag_ms: Some(0),
            ..Default::default()
        },
        ..Default::default()
    })
    .await;
    let b = begun(&h).await;
    h.engine.teardown(&b.id).await.unwrap();
    assert_eq!(h.status(&b.id).await, OptimizationStatus::Completed);
    let cr = h
        .engine
        .teardown_report(&b.id)
        .await
        .unwrap()
        .unwrap()
        .cost_reconcile
        .unwrap();
    assert_eq!(cr.status, "error");
    assert_eq!(cr.reason.as_deref(), Some("ce down"));
}
