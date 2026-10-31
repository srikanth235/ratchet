//! Port of `packages/core/src/sandbox/lifecycle.test.ts`, running the lifecycle against the real SQLite ledger.

mod common;

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use common::*;
use ratchet_core::enums::{OptimizationStatus, SandboxProviderKind, SandboxStatus};
use ratchet_core::ledger::RecordSandboxInput;
use ratchet_core::ports::mock::{MockSandboxProvider, sample_sandbox_outputs};
use ratchet_core::ports::{
    PortError, PortResult, SandboxInput, SandboxOutputs, SandboxPlan, SandboxProvider,
    SandboxStateStatus,
};
use ratchet_core::rows::Optimization;
use ratchet_core::sandbox::{
    MemorySandboxInputStore, SandboxBlockers, SandboxLifecycle, SandboxLifecycleError,
    SandboxLifecycleErrorCode, SandboxLifecycleOptions,
};
use ratchet_ledger::{CreateOptimizationInput, Ledger};
use tokio::sync::Notify;

fn input() -> SandboxInput {
    SandboxInput {
        sandbox_id: "sbx-1".into(),
        region: "us-east-1".into(),
        allowed_instance_types: vec!["c7i.large".into()],
        max_instances_per_agent: 2,
        max_instance_lifetime_hours: 4.0,
        tags: None,
        environment: None,
    }
}

fn outputs() -> SandboxOutputs {
    let mut o = sample_sandbox_outputs("sbx");
    o.artifacts_bucket = "a".into();
    o.sealed_bucket = "s".into();
    o
}

/// A ledger to peek at while `apply` runs, and where to record the status it saw.
type Watch = (Ledger, Arc<Mutex<Vec<SandboxStatus>>>);

/// A provider with the knobs the TypeScript test class had: call log, failures, a gate, and a peek at the ledger
/// status while `apply` runs.
struct TestProvider {
    calls: Mutex<Vec<String>>,
    inputs: Mutex<Vec<SandboxInput>>,
    fail_apply: Mutex<Option<PortError>>,
    fail_destroy: Mutex<Option<PortError>>,
    gate: Mutex<Option<Arc<Notify>>>,
    watch: Mutex<Option<Watch>>,
}

impl TestProvider {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::default(),
            inputs: Mutex::default(),
            fail_apply: Mutex::default(),
            fail_destroy: Mutex::default(),
            gate: Mutex::default(),
            watch: Mutex::default(),
        })
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
}

#[async_trait]
impl SandboxProvider for TestProvider {
    async fn plan(&self, _i: &SandboxInput) -> PortResult<SandboxPlan> {
        self.calls.lock().unwrap().push("plan".into());
        Ok(SandboxPlan {
            changes: vec![],
            summary: "Plan: 1 to add, 0 to change, 0 to destroy.".into(),
        })
    }

    async fn apply(&self, i: &SandboxInput) -> PortResult<SandboxOutputs> {
        self.calls.lock().unwrap().push("apply".into());
        self.inputs.lock().unwrap().push(i.clone());
        let watch = self.watch.lock().unwrap().clone();
        if let Some((ledger, seen)) = watch {
            let status = ledger.get_sandbox("sbx-1").await.unwrap().unwrap().status;
            seen.lock().unwrap().push(status);
        }
        let gate = self.gate.lock().unwrap().clone();
        if let Some(g) = gate {
            g.notified().await;
        }
        if let Some(e) = self.fail_apply.lock().unwrap().clone() {
            return Err(e);
        }
        Ok(outputs())
    }

    async fn destroy(&self, i: &SandboxInput) -> PortResult<()> {
        self.calls.lock().unwrap().push("destroy".into());
        self.inputs.lock().unwrap().push(i.clone());
        match self.fail_destroy.lock().unwrap().clone() {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    async fn outputs(&self, _id: &str) -> PortResult<Option<SandboxOutputs>> {
        Ok(None)
    }
}

struct Blockers(Mutex<Result<Vec<String>, String>>);

#[async_trait]
impl SandboxBlockers for Blockers {
    async fn blockers(&self, _id: &str, _o: &[Optimization]) -> Result<Vec<String>, PortError> {
        self.0.lock().unwrap().clone().map_err(PortError::Failed)
    }
}

fn lifecycle_over(
    ledger: &Ledger,
    provider: Arc<dyn SandboxProvider>,
    blockers: Option<Arc<dyn SandboxBlockers>>,
) -> SandboxLifecycle {
    SandboxLifecycle::new(SandboxLifecycleOptions {
        ledger: Arc::new(ledger.clone()),
        provider,
        kind: SandboxProviderKind::Mock,
        blockers,
        inputs: Some(Arc::new(MemorySandboxInputStore::default())),
    })
}

async fn setup(
    blockers: Option<Arc<dyn SandboxBlockers>>,
) -> (Ledger, Arc<TestProvider>, SandboxLifecycle) {
    let ledger = ledger().await;
    let provider = TestProvider::new();
    let lifecycle = lifecycle_over(&ledger, provider.clone(), blockers);
    (ledger, provider, lifecycle)
}

async fn add_optimization(ledger: &Ledger, id: &str, status: OptimizationStatus) {
    let mut o = CreateOptimizationInput::new(
        "r",
        "main",
        "m",
        ratchet_core::enums::Direction::Minimize,
        0.1,
        10.0,
        60,
        "x",
    );
    o.id = Some(id.into());
    o.status = Some(status);
    o.sandbox_id = Some("sbx-1".into());
    ledger.create_optimization(o).await.unwrap();
}

fn code(e: &SandboxLifecycleError) -> Option<SandboxLifecycleErrorCode> {
    e.code()
}

// "reports an absent sandbox, then walks planned -> applying -> applied with outputs and account"
#[tokio::test]
async fn reports_an_absent_sandbox_then_walks_planned_applying_applied() {
    let (ledger, provider, lifecycle) = setup(None).await;
    let s = lifecycle.state("sbx-1").await.unwrap();
    assert_eq!((s.status, s.outputs), (SandboxStateStatus::Absent, None));
    let plan = lifecycle.plan(&input()).await.unwrap();
    assert!(plan.summary.contains("1 to add"));
    let s = lifecycle.state("sbx-1").await.unwrap();
    assert_eq!(
        (s.status, s.provider, s.region.as_deref()),
        (
            SandboxStateStatus::Planned,
            Some(SandboxProviderKind::Mock),
            Some("us-east-1")
        )
    );

    let seen = Arc::new(Mutex::new(Vec::new()));
    *provider.watch.lock().unwrap() = Some((ledger.clone(), seen.clone()));
    let out = lifecycle.apply(&input()).await.unwrap();
    assert_eq!(out, outputs());
    assert_eq!(*seen.lock().unwrap(), [SandboxStatus::Applying]);
    let s = lifecycle.state("sbx-1").await.unwrap();
    assert_eq!(
        (s.status, s.account_id.as_deref(), s.outputs),
        (
            SandboxStateStatus::Applied,
            Some("123456789012"),
            Some(outputs())
        )
    );
    assert!(s.applied_at.is_some());
}

// "records a failed apply, keeps the error, and can be retried"
#[tokio::test]
async fn records_a_failed_apply_keeps_the_error_and_can_be_retried() {
    let (_l, provider, lifecycle) = setup(None).await;
    *provider.fail_apply.lock().unwrap() = Some(PortError::failed("tofu apply failed"));
    let e = lifecycle.apply(&input()).await.unwrap_err();
    assert_eq!(e.to_string(), "tofu apply failed");
    let s = lifecycle.state("sbx-1").await.unwrap();
    assert_eq!(
        (s.status, s.last_error.as_deref()),
        (SandboxStateStatus::Failed, Some("tofu apply failed"))
    );
    *provider.fail_apply.lock().unwrap() = None;
    lifecycle.apply(&input()).await.unwrap();
    let s = lifecycle.state("sbx-1").await.unwrap();
    assert_eq!(
        (s.status, s.last_error),
        (SandboxStateStatus::Applied, None)
    );
}

// "destroys with the input it was applied with, clears outputs, and is idempotent"
#[tokio::test]
async fn destroys_with_the_input_it_was_applied_with_clears_outputs_and_is_idempotent() {
    let (_l, provider, lifecycle) = setup(None).await;
    let mut three = input();
    three.max_instances_per_agent = 3;
    lifecycle.apply(&three).await.unwrap();
    lifecycle.destroy("sbx-1").await.unwrap();
    assert_eq!(
        provider
            .inputs
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .max_instances_per_agent,
        3
    );
    let s = lifecycle.state("sbx-1").await.unwrap();
    assert_eq!((s.status, s.outputs), (SandboxStateStatus::Destroyed, None));
    assert!(s.destroyed_at.is_some());
    lifecycle.destroy("sbx-1").await.unwrap();
    assert_eq!(
        provider.calls().iter().filter(|c| *c == "destroy").count(),
        1
    );
    // a destroyed sandbox can be applied again
    lifecycle.apply(&input()).await.unwrap();
    assert_eq!(
        lifecycle.state("sbx-1").await.unwrap().status,
        SandboxStateStatus::Applied
    );
}

// "refuses to destroy something never applied, or unknown"
#[tokio::test]
async fn refuses_to_destroy_something_never_applied_or_unknown() {
    let (_l, _p, lifecycle) = setup(None).await;
    assert_eq!(
        code(&lifecycle.destroy("nope").await.unwrap_err()),
        Some(SandboxLifecycleErrorCode::NotFound)
    );
    lifecycle.plan(&input()).await.unwrap();
    assert_eq!(
        code(&lifecycle.destroy("sbx-1").await.unwrap_err()),
        Some(SandboxLifecycleErrorCode::InvalidState)
    );
}

// "refuses to destroy while an optimization in the sandbox is running, and changes nothing"
#[tokio::test]
async fn refuses_to_destroy_while_an_optimization_in_the_sandbox_is_running() {
    let (ledger, provider, lifecycle) = setup(None).await;
    lifecycle.apply(&input()).await.unwrap();
    add_optimization(&ledger, "opt-1", OptimizationStatus::Running).await;
    let err = lifecycle.destroy("sbx-1").await.unwrap_err();
    assert!(
        matches!(&err, SandboxLifecycleError::SandboxInUse { optimization_ids, .. } if optimization_ids == &["opt-1".to_owned()])
    );
    assert_eq!(code(&err), Some(SandboxLifecycleErrorCode::SandboxInUse));
    assert!(err.to_string().contains("opt-1"));
    assert!(!provider.calls().contains(&"destroy".to_owned()));
    assert_eq!(
        lifecycle.state("sbx-1").await.unwrap().status,
        SandboxStateStatus::Applied
    );
    ledger
        .set_optimization_status(&"opt-1".into(), OptimizationStatus::Stopped, None)
        .await
        .unwrap();
    lifecycle.destroy("sbx-1").await.unwrap();
    assert_eq!(
        lifecycle.state("sbx-1").await.unwrap().status,
        SandboxStateStatus::Destroyed
    );
}

// "refuses while the orphan check reports leftovers, and runs it fail-closed"
#[tokio::test]
async fn refuses_while_the_orphan_check_reports_leftovers_and_runs_it_fail_closed() {
    let blockers = Arc::new(Blockers(Mutex::new(Ok(vec!["i-0abc running".to_owned()]))));
    let (_l, provider, lifecycle) = setup(Some(blockers.clone())).await;
    lifecycle.apply(&input()).await.unwrap();
    assert_eq!(
        code(&lifecycle.destroy("sbx-1").await.unwrap_err()),
        Some(SandboxLifecycleErrorCode::Orphans)
    );
    assert!(!provider.calls().contains(&"destroy".to_owned()));
    assert_eq!(
        lifecycle.state("sbx-1").await.unwrap().status,
        SandboxStateStatus::Applied
    );
    *blockers.0.lock().unwrap() = Ok(vec![]);
    lifecycle.destroy("sbx-1").await.unwrap();
    assert_eq!(
        lifecycle.state("sbx-1").await.unwrap().status,
        SandboxStateStatus::Destroyed
    );

    // a failing orphan check blocks the destroy too
    let failing = Arc::new(Blockers(Mutex::new(Err("listTagged failed".to_owned()))));
    let (_l2, _p2, l2) = setup(Some(failing)).await;
    l2.apply(&input()).await.unwrap();
    assert_eq!(
        l2.destroy("sbx-1").await.unwrap_err().to_string(),
        "listTagged failed"
    );
    assert_eq!(
        l2.state("sbx-1").await.unwrap().status,
        SandboxStateStatus::Applied
    );
}

// "restores the previous status when the provider's own teardown guard refuses"
#[tokio::test]
async fn restores_the_previous_status_when_the_providers_own_teardown_guard_refuses() {
    let (_l, provider, lifecycle) = setup(None).await;
    lifecycle.apply(&input()).await.unwrap();
    *provider.fail_destroy.lock().unwrap() = Some(PortError::TeardownBlocked(
        "refusing to destroy sandbox: 1 runtime agent role(s) still exist".into(),
    ));
    assert_eq!(
        code(&lifecycle.destroy("sbx-1").await.unwrap_err()),
        Some(SandboxLifecycleErrorCode::Orphans)
    );
    assert_eq!(
        lifecycle.state("sbx-1").await.unwrap().status,
        SandboxStateStatus::Applied
    );
}

// "marks a destroy that failed inside the provider as failed (state unknown) and allows a retry"
#[tokio::test]
async fn marks_a_destroy_that_failed_inside_the_provider_as_failed_and_allows_a_retry() {
    let (_l, provider, lifecycle) = setup(None).await;
    lifecycle.apply(&input()).await.unwrap();
    *provider.fail_destroy.lock().unwrap() = Some(PortError::failed("tofu destroy failed"));
    assert_eq!(
        lifecycle.destroy("sbx-1").await.unwrap_err().to_string(),
        "tofu destroy failed"
    );
    let s = lifecycle.state("sbx-1").await.unwrap();
    assert_eq!(
        (s.status, s.last_error.as_deref()),
        (SandboxStateStatus::Failed, Some("tofu destroy failed"))
    );
    *provider.fail_destroy.lock().unwrap() = None;
    lifecycle.destroy("sbx-1").await.unwrap();
    assert_eq!(
        lifecycle.state("sbx-1").await.unwrap().status,
        SandboxStateStatus::Destroyed
    );
}

// "rejects a second concurrent operation on the same sandbox"
#[tokio::test]
async fn rejects_a_second_concurrent_operation_on_the_same_sandbox() {
    let (_l, provider, lifecycle) = setup(None).await;
    let gate = Arc::new(Notify::new());
    *provider.gate.lock().unwrap() = Some(gate.clone());
    let lifecycle = Arc::new(lifecycle);
    let first = {
        let lc = lifecycle.clone();
        tokio::spawn(async move { lc.apply(&input()).await })
    };
    // wait until the first apply is inside the provider
    while !provider.calls().contains(&"apply".to_owned()) {
        tokio::task::yield_now().await;
    }
    let second = lifecycle.apply(&input()).await.unwrap_err();
    assert_eq!(code(&second), Some(SandboxLifecycleErrorCode::InvalidState));
    gate.notify_one();
    first.await.unwrap().unwrap();
    assert_eq!(
        lifecycle.state("sbx-1").await.unwrap().status,
        SandboxStateStatus::Applied
    );
}

// "recover() marks interrupted operations as failed"
#[tokio::test]
async fn recover_marks_interrupted_operations_as_failed() {
    let (ledger, _p, lifecycle) = setup(None).await;
    for (id, status) in [
        ("sbx-1", SandboxStatus::Applying),
        ("sbx-2", SandboxStatus::Destroying),
        ("sbx-3", SandboxStatus::Applied),
    ] {
        let mut r = RecordSandboxInput::new(id, SandboxProviderKind::Mock, "us-east-1");
        r.status = Some(status);
        ledger.record_sandbox(r).await.unwrap();
    }
    let mut fixed = lifecycle.recover().await.unwrap();
    fixed.sort();
    assert_eq!(fixed, ["sbx-1", "sbx-2"]);
    let s1 = lifecycle.state("sbx-1").await.unwrap();
    assert_eq!(s1.status, SandboxStateStatus::Failed);
    assert!(s1.last_error.unwrap().contains("interrupted"));
    assert_eq!(
        lifecycle.state("sbx-3").await.unwrap().status,
        SandboxStateStatus::Applied
    );
}

// ---------------------------------------------------------------- beyond the TypeScript cases

#[tokio::test]
async fn the_core_mock_provider_drives_the_same_lifecycle() {
    let ledger = ledger().await;
    let provider = Arc::new(MockSandboxProvider::new(outputs()));
    let lifecycle = lifecycle_over(&ledger, provider.clone(), None);
    lifecycle.apply(&input()).await.unwrap();
    lifecycle.destroy("sbx-1").await.unwrap();
    assert_eq!(provider.calls(), ["apply", "destroy"]);
    // the busy flag is released after every operation, successful or not
    provider.fail_apply(Some(PortError::failed("x")));
    assert!(lifecycle.apply(&input()).await.is_err());
    provider.fail_apply(None);
    lifecycle.apply(&input()).await.unwrap();
}
