//! Port of `packages/aws/src/cost.test.ts`.

use std::sync::{Arc, Mutex};

use ratchet_sandbox::cost::{
    BudgetGuard, BudgetState, BudgetStatus, CostMeter, CostMeterOptions, Crossed, MeteredCategory,
};
use ratchet_sandbox::domain::{
    AgentIdentity, AgentKind, BenchLaunchSpec, BenchPurpose, RunnerLaunchSpec,
};
use ratchet_sandbox::ports::Compute;
use ratchet_sandbox::{ManualClock, MockCloudCompute, StaticPriceBook};

const HOUR: i64 = 3_600_000;

struct Setup {
    clock: Arc<ManualClock>,
    cloud: Arc<MockCloudCompute>,
    meter: CostMeter,
    spec: RunnerLaunchSpec,
}

async fn setup() -> Setup {
    let clock = Arc::new(ManualClock::new(10 * HOUR));
    let cloud = Arc::new(MockCloudCompute::with_clock(clock.clone()));
    let prices = Arc::new(StaticPriceBook::with(&[
        ("c7i.large", 0.1),
        ("c7i.2xlarge", 0.4),
        ("m7i.large", 0.2),
    ]));
    let meter = CostMeter::new(CostMeterOptions::new(cloud.clone(), prices).clock(clock.clone()));
    let id: AgentIdentity = cloud.create_agent_identity("o1", "a1").await.unwrap();
    let spec = RunnerLaunchSpec {
        optimization_id: "o1".into(),
        agent_id: "a1".into(),
        kind: AgentKind::Mock,
        instance_type: "c7i.large".into(),
        image_ref: "ami".into(),
        identity: id,
        secrets: vec![],
        repo_url: "r".into(),
        branch: "b".into(),
    };
    Setup {
        clock,
        cloud,
        meter,
        spec,
    }
}

fn bench(instance_type: &str) -> BenchLaunchSpec {
    BenchLaunchSpec {
        optimization_id: "o1".into(),
        instance_type: instance_type.into(),
        image_ref: "a".into(),
        purpose: BenchPurpose::Bench,
    }
}

fn close(a: f64, b: f64) {
    assert!((a - b).abs() < 1e-9, "{a} !~ {b}");
}

#[tokio::test]
async fn computes_live_spend_per_optimization_agent_role_from_lifetimes() {
    let s = setup().await;
    s.cloud.launch_runner(&s.spec).await.unwrap(); // 0.1/h
    s.cloud.simulate_experiment("o1", "a1", "m7i.large"); // 0.2/h
    s.cloud
        .launch_bench_instance(&bench("c7i.2xlarge"))
        .await
        .unwrap(); // 0.4/h
    s.clock.advance(HOUR * 3 / 2);
    let snap = s.meter.snapshot(Some("o1")).await.unwrap();
    let o = &snap.by_optimization["o1"];
    close(o.by_category[&MeteredCategory::AgentRunner], 0.15);
    close(o.by_category[&MeteredCategory::Experiment], 0.3);
    close(o.by_category[&MeteredCategory::BenchRunner], 0.6);
    close(o.by_agent["a1"], 0.45);
    close(snap.total_usd, 1.05);
    assert!(snap.unpriced.is_empty());
}

#[tokio::test]
async fn stops_accruing_at_termination_and_applies_the_60_s_minimum() {
    let s = setup().await;
    let r = s.cloud.launch_runner(&s.spec).await.unwrap();
    s.clock.advance(10_000);
    s.cloud.terminate(&[r.id]).await.unwrap();
    s.clock.advance(5 * HOUR);
    let snap = s.meter.snapshot(None).await.unwrap();
    assert_eq!(snap.instances[0].billed_seconds, 60.0);
    close(snap.total_usd, (60.0 / 3600.0) * 0.1);
}

#[tokio::test]
async fn collect_emits_only_new_spend_ledger_shaped_aggregated_per_opt_agent_category() {
    let s = setup().await;
    s.cloud.launch_runner(&s.spec).await.unwrap();
    s.cloud.simulate_experiment("o1", "a1", "c7i.large");
    s.cloud.simulate_experiment("o1", "a1", "c7i.large");
    s.clock.advance(HOUR);
    let first = s.meter.collect(None).await.unwrap();
    assert_eq!(first.len(), 2);
    let exp = first
        .iter()
        .find(|d| d.category == MeteredCategory::Experiment)
        .unwrap();
    assert_eq!(
        (
            exp.optimization_id.as_str(),
            exp.agent_id.as_deref(),
            exp.estimated,
            exp.source.as_str(),
            exp.at
        ),
        ("o1", Some("a1"), true, "cost-meter", 11 * HOUR)
    );
    close(exp.usd, 0.2);
    close(
        first
            .iter()
            .find(|d| d.category == MeteredCategory::AgentRunner)
            .unwrap()
            .usd,
        0.1,
    );

    assert!(s.meter.collect(None).await.unwrap().is_empty()); // nothing elapsed
    s.clock.advance(HOUR / 2);
    let second = s.meter.collect(None).await.unwrap();
    close(second.iter().map(|d| d.usd).sum(), 0.15);
    // sum of deltas == cumulative snapshot
    let total: f64 = first.iter().chain(second.iter()).map(|d| d.usd).sum();
    close(s.meter.snapshot(None).await.unwrap().total_usd, total);
}

#[tokio::test]
async fn does_not_double_count_instances_that_terminate_between_collects() {
    let s = setup().await;
    let r = s.cloud.launch_runner(&s.spec).await.unwrap();
    s.clock.advance(HOUR);
    let a = s.meter.collect(None).await.unwrap();
    s.clock.advance(HOUR / 2);
    s.cloud.terminate(&[r.id]).await.unwrap();
    s.clock.advance(3 * HOUR);
    let b = s.meter.collect(None).await.unwrap();
    close(a[0].usd + b[0].usd, 0.15);
    assert!(s.meter.collect(None).await.unwrap().is_empty());
}

#[tokio::test]
async fn bench_spend_has_no_agent_id_unpriced_types_are_flagged_not_silently_priced() {
    let s = setup().await;
    s.cloud
        .launch_bench_instance(&bench("x9.huge"))
        .await
        .unwrap();
    s.clock.advance(HOUR);
    let snap = s.meter.snapshot(None).await.unwrap();
    assert_eq!(snap.unpriced, ["x9.huge"]);
    assert_eq!(snap.total_usd, 0.0);
    s.cloud
        .launch_bench_instance(&bench("c7i.2xlarge"))
        .await
        .unwrap();
    let d = s
        .meter
        .collect(None)
        .await
        .unwrap()
        .into_iter()
        .find(|x| x.category == MeteredCategory::BenchRunner)
        .unwrap();
    assert_eq!(d.agent_id, None);
    // the ledger-shaped JSON has no agentId key at all
    assert!(
        !serde_json::to_value(&d)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("agentId")
    );
}

#[tokio::test]
async fn stopped_instance_without_timestamp_stops_accruing_at_first_observation() {
    let s = setup().await;
    let r = s.cloud.launch_runner(&s.spec).await.unwrap();
    s.clock.advance(HOUR);
    s.cloud.simulate_stopped(&r.id);
    s.clock.advance(HOUR); // stop was HOUR ago; mock stamps terminatedAt so billing ends there
    close(s.meter.snapshot(None).await.unwrap().total_usd, 0.1);
}

// ---- BudgetGuard --------------------------------------------------------------------------------------

#[test]
fn warns_at_80_percent_trips_at_100_percent_each_exactly_once() {
    let events: Arc<Mutex<Vec<&'static str>>> = Arc::default();
    let (w, t) = (events.clone(), events.clone());
    let mut g = BudgetGuard::new(100.0)
        .unwrap()
        .on_warn(move |_| w.lock().unwrap().push("warn"))
        .on_trip(move |_| t.lock().unwrap().push("trip"));
    let st = |s: &BudgetStatus| (s.state, s.crossed);
    assert_eq!(st(&g.update(79.99)), (BudgetState::Ok, None));
    let at80 = g.update(80.0);
    assert_eq!(
        (at80.state, at80.crossed, at80.fraction),
        (BudgetState::Warn, Some(Crossed::Warn), 0.8)
    );
    assert_eq!(st(&g.update(90.0)), (BudgetState::Warn, None));
    assert_eq!(
        st(&g.update(100.0)),
        (BudgetState::Tripped, Some(Crossed::Trip))
    );
    assert_eq!(st(&g.update(150.0)), (BudgetState::Tripped, None));
    assert_eq!(g.update(10.0).state, BudgetState::Tripped); // never un-trips
    assert_eq!(*events.lock().unwrap(), ["warn", "trip"]);
    assert_eq!(g.current(), BudgetState::Tripped);
}

#[test]
fn jumping_straight_past_100_percent_trips_without_a_warn() {
    let events: Arc<Mutex<Vec<&'static str>>> = Arc::default();
    let (w, t) = (events.clone(), events.clone());
    let mut g = BudgetGuard::new(10.0)
        .unwrap()
        .on_warn(move |_| w.lock().unwrap().push("warn"))
        .on_trip(move |_| t.lock().unwrap().push("trip"));
    assert_eq!(g.update(12.0).crossed, Some(Crossed::Trip));
    assert_eq!(*events.lock().unwrap(), ["trip"]);
}

#[test]
fn rejects_a_non_positive_budget_custom_warn_fraction() {
    assert!(BudgetGuard::new(0.0).is_err());
    assert!(BudgetGuard::new(-1.0).is_err());
    assert!(BudgetGuard::new(f64::NAN).is_err());
    assert_eq!(
        BudgetGuard::new(10.0)
            .unwrap()
            .warn_fraction(0.5)
            .update(5.0)
            .crossed,
        Some(Crossed::Warn)
    );
}
