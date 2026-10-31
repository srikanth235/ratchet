//! `packages/engine/src/sandbox.test.ts`

use ratchet_core::enums::{Mode, SandboxProviderKind, SandboxStatus};
use ratchet_core::ledger::RecordSandboxInput;
use ratchet_core::ports::{GitHost as _, PlanAction, PortError, SandboxInput, SandboxStateStatus};
use ratchet_engine::{EngineError, EngineErrorCode as C, NOT_CONFIGURED};

use crate::common::*;

const SBX: &str = "ratchet-sandbox";

fn input() -> SandboxInput {
    SandboxInput {
        sandbox_id: SBX.into(),
        region: "us-east-1".into(),
        allowed_instance_types: vec!["c7i.xlarge".into()],
        max_instances_per_agent: 2,
        max_instance_lifetime_hours: 4.0,
        tags: None,
    }
}

fn live() -> HarnessOptions {
    HarnessOptions {
        mode: Some(Mode::Live),
        ..Default::default()
    }
}

async fn draft(h: &Harness, sandbox_id: Option<&str>) -> Result<String, EngineError> {
    h.engine
        .create_optimization(CreateOptimizationInput {
            repo_url: h.git.remote_url(),
            base_ref: "main".into(),
            agents: vec![Codex],
            budget_usd: 10.0,
            time_limit_seconds: 600,
            plateau_evals: 3,
            sandbox_id: sandbox_id.map(str::to_owned),
            ..Default::default()
        })
        .await
}

fn sbx_begin() -> BeginOptions {
    BeginOptions {
        sandbox_id: Some(SBX.into()),
        ..Default::default()
    }
}

fn code<T: std::fmt::Debug>(r: Result<T, EngineError>) -> C {
    r.expect_err("expected an error").code
}

// ------------------------------------------------------------------------------------------ sandbox lifecycle

#[tokio::test]
async fn plans_applies_and_records_planned_applying_applied_with_the_outputs_in_the_ledger() {
    let h = Harness::new(live()).await;
    assert_eq!(
        h.engine.sandbox_state(SBX).await.unwrap().status,
        SandboxStateStatus::Absent
    );

    let plan = h.engine.plan_sandbox(&input()).await.unwrap();
    assert_eq!(plan.summary, "Plan: 1 to add, 0 to change, 0 to destroy.");
    assert_eq!(plan.changes[0].action, PlanAction::Create);
    assert_eq!(plan.changes[0].address, "aws_s3_bucket.artifacts");
    let s = h.engine.sandbox_state(SBX).await.unwrap();
    assert_eq!(s.status, SandboxStateStatus::Planned);
    assert_eq!(s.provider, Some(SandboxProviderKind::Aws));
    assert_eq!(s.region.as_deref(), Some("us-east-1"));
    assert!(s.outputs.is_none());

    *h.sandbox.observe.lock().unwrap() = Some(h.ledger.clone());
    let out = h.engine.apply_sandbox(&input()).await.unwrap();
    assert_eq!(*h.sandbox.status_during_apply.lock().unwrap(), ["applying"]);
    // (the fake provider's sample outputs name the bucket differently from the TS fake; same shape)
    assert_eq!(out.artifacts_bucket, "ratchet-artifacts-ratchet-sandbox");

    let state = h.engine.sandbox_state(SBX).await.unwrap();
    assert_eq!(state.status, SandboxStateStatus::Applied);
    assert_eq!(state.account_id.as_deref(), Some("123456789012"));
    assert_eq!(state.outputs.as_ref(), Some(&out));
    assert!(state.applied_at.is_some());
    let stored = h
        .ledger
        .get_sandbox(SBX)
        .await
        .unwrap()
        .unwrap()
        .outputs
        .unwrap();
    assert_eq!(
        stored["artifactsBucket"],
        "ratchet-artifacts-ratchet-sandbox"
    );
    assert_eq!(stored["network"]["subnetId"], out.network.subnet_id);
    // a re-plan of an applied sandbox shows nothing to do
    let replan = h.engine.plan_sandbox(&input()).await.unwrap();
    assert!(replan.changes.iter().all(|c| c.action == PlanAction::NoOp));
}

#[tokio::test]
async fn records_a_failed_apply_and_recovers_on_retry() {
    let h = Harness::new(live()).await;
    *h.sandbox.fail_apply.lock().unwrap() = Some(PortError::failed("tofu apply failed (exit 1)"));
    let e = h.engine.apply_sandbox(&input()).await.unwrap_err();
    assert!(e.message.contains("tofu apply failed"), "{}", e.message);
    let s = h.engine.sandbox_state(SBX).await.unwrap();
    assert_eq!(s.status, SandboxStateStatus::Failed);
    assert_eq!(s.last_error.as_deref(), Some("tofu apply failed (exit 1)"));
    *h.sandbox.fail_apply.lock().unwrap() = None;
    h.engine.apply_sandbox(&input()).await.unwrap();
    assert_eq!(
        h.engine.sandbox_state(SBX).await.unwrap().status,
        SandboxStateStatus::Applied
    );
}

#[tokio::test]
async fn marks_an_apply_that_a_crash_interrupted_as_failed_when_the_engine_restarts() {
    let mut h = Harness::new(live()).await;
    h.ledger
        .record_sandbox(RecordSandboxInput {
            id: SBX.into(),
            provider: SandboxProviderKind::Aws,
            region: "us-east-1".into(),
            account_id: None,
            status: Some(SandboxStatus::Applying),
            created_at: None,
        })
        .await
        .unwrap();
    h.restart(false).await;
    let s = h.engine.sandbox_state(SBX).await.unwrap();
    assert_eq!(s.status, SandboxStateStatus::Failed);
    assert!(s.last_error.unwrap_or_default().contains("interrupted"));
    h.engine.apply_sandbox(&input()).await.unwrap();
    assert_eq!(
        h.engine.sandbox_state(SBX).await.unwrap().status,
        SandboxStateStatus::Applied
    );
}

#[tokio::test]
async fn destroys_with_the_input_it_was_applied_with_applied_destroying_destroyed_outputs_cleared()
{
    let h = Harness::new(live()).await;
    h.engine.apply_sandbox(&input()).await.unwrap();
    *h.sandbox.observe.lock().unwrap() = Some(h.ledger.clone());
    h.engine.destroy_sandbox(SBX).await.unwrap();
    assert_eq!(
        *h.sandbox.status_during_destroy.lock().unwrap(),
        ["destroying"]
    );
    assert_eq!(*h.sandbox.destroy_inputs.lock().unwrap(), [input()]);
    let s = h.engine.sandbox_state(SBX).await.unwrap();
    assert_eq!(s.status, SandboxStateStatus::Destroyed);
    assert!(s.outputs.is_none());
    assert!(!h.sandbox.applied.lock().unwrap().contains_key(SBX));
}

#[tokio::test]
async fn a_destroy_that_fails_inside_the_provider_leaves_the_sandbox_failed_and_can_be_retried() {
    let h = Harness::new(live()).await;
    h.engine.apply_sandbox(&input()).await.unwrap();
    *h.sandbox.fail_destroy.lock().unwrap() = Some(PortError::failed("tofu destroy failed"));
    let e = h.engine.destroy_sandbox(SBX).await.unwrap_err();
    assert!(e.message.contains("tofu destroy failed"));
    let s = h.engine.sandbox_state(SBX).await.unwrap();
    assert_eq!(s.status, SandboxStateStatus::Failed);
    assert_eq!(s.last_error.as_deref(), Some("tofu destroy failed"));
    *h.sandbox.fail_destroy.lock().unwrap() = None;
    h.engine.destroy_sandbox(SBX).await.unwrap();
    assert_eq!(
        h.engine.sandbox_state(SBX).await.unwrap().status,
        SandboxStateStatus::Destroyed
    );
}

#[tokio::test]
async fn keeps_the_ledger_state_across_an_engine_restart() {
    let mut h = Harness::new(live()).await;
    h.engine.apply_sandbox(&input()).await.unwrap();
    h.restart(false).await;
    assert_eq!(
        h.engine.sandbox_state(SBX).await.unwrap().status,
        SandboxStateStatus::Applied
    );
    h.engine.destroy_sandbox(SBX).await.unwrap(); // the input survives in the engine's kv table
    assert_eq!(
        h.engine.sandbox_state(SBX).await.unwrap().status,
        SandboxStateStatus::Destroyed
    );
}

// ------------------------------------------------------------------------------------ optimizations and the sandbox

#[tokio::test]
async fn live_mode_refuses_to_create_or_start_an_optimization_without_an_applied_sandbox() {
    let h = Harness::new(live()).await;
    assert_eq!(code(draft(&h, None).await), C::SandboxNotApplied);
    assert_eq!(code(draft(&h, Some(SBX)).await), C::SandboxNotApplied);
    h.engine.plan_sandbox(&input()).await.unwrap(); // planned is not applied
    let e = draft(&h, Some(SBX)).await.unwrap_err();
    assert!(e.message.contains("planned"), "{}", e.message);
    assert!(h.ledger.list_optimizations(None).await.unwrap().is_empty());

    h.engine.apply_sandbox(&input()).await.unwrap();
    let id = draft(&h, Some(SBX)).await.unwrap();
    assert_eq!(
        h.ledger
            .get_optimization(&oid(&id))
            .await
            .unwrap()
            .unwrap()
            .sandbox_id
            .as_ref()
            .map(|s| s.as_str()),
        Some(SBX)
    );

    // The sandbox disappears between create and start: start refuses and nothing is launched.
    h.engine.destroy_sandbox(SBX).await.unwrap();
    assert_eq!(code(h.engine.start(&id).await), C::SandboxNotApplied);
    assert_eq!(h.status(&id).await, OptimizationStatus::Draft);
    assert!(h.log.all().iter().all(|l| !l.starts_with("compute.")));
}

#[tokio::test]
async fn live_mode_links_the_sandbox_to_the_optimization_it_runs_in() {
    let h = Harness::new(live()).await;
    h.engine.apply_sandbox(&input()).await.unwrap();
    let b = h.begin(sbx_begin()).await.unwrap();
    let o = h
        .ledger
        .get_optimization(&oid(&b.id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(o.sandbox_id.as_ref().map(|s| s.as_str()), Some(SBX));
    assert_eq!(o.status, OptimizationStatus::Running);
    let refs = h.engine.sandbox_state(SBX).await.unwrap().optimizations;
    assert_eq!(refs.len(), 1);
    assert_eq!(
        (refs[0].id.as_str(), refs[0].status),
        (b.id.as_str(), OptimizationStatus::Running)
    );
}

#[tokio::test]
async fn simulation_does_not_need_a_sandbox_and_links_one_only_once_it_is_applied() {
    let h = Harness::new(Default::default()).await;
    let a = draft(&h, Some(SBX)).await.unwrap();
    assert_eq!(
        h.ledger
            .get_optimization(&oid(&a))
            .await
            .unwrap()
            .unwrap()
            .sandbox_id,
        None
    );
    h.engine.apply_sandbox(&input()).await.unwrap();
    let b = h
        .engine
        .create_optimization(CreateOptimizationInput {
            id: Some("opt-b".into()),
            repo_url: h.git.remote_url(),
            base_ref: "main".into(),
            agents: vec![Codex],
            budget_usd: 10.0,
            time_limit_seconds: 600,
            plateau_evals: 3,
            sandbox_id: Some(SBX.into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        h.ledger
            .get_optimization(&oid(&b))
            .await
            .unwrap()
            .unwrap()
            .sandbox_id
            .as_ref()
            .map(|s| s.as_str()),
        Some(SBX)
    );
    assert_eq!(code(draft(&h, Some("Not Valid")).await), C::InvalidId);
}

#[tokio::test]
async fn refuses_to_destroy_while_an_optimization_in_the_sandbox_is_running_then_allows_it_after_teardown()
 {
    let h = Harness::new(live()).await;
    h.engine.apply_sandbox(&input()).await.unwrap();
    let b = h.begin(sbx_begin()).await.unwrap();
    let e = h.engine.destroy_sandbox(SBX).await.unwrap_err();
    assert_eq!(e.code, C::SandboxInUse);
    assert!(e.message.contains(&b.id));
    assert_eq!(
        h.engine.sandbox_state(SBX).await.unwrap().status,
        SandboxStateStatus::Applied
    );
    assert!(!h.log.all().contains(&format!("sandbox.destroy:{SBX}")));

    h.engine.stop(&b.id, None).await.unwrap();
    // stopped, but its agent identities still exist: teardown first (the existing orphan check)
    assert_eq!(code(h.engine.destroy_sandbox(SBX).await), C::Orphans);
    assert_eq!(
        h.engine.sandbox_state(SBX).await.unwrap().status,
        SandboxStateStatus::Applied
    );

    h.engine.teardown(&b.id).await.unwrap();
    h.engine.destroy_sandbox(SBX).await.unwrap();
    assert_eq!(
        h.engine.sandbox_state(SBX).await.unwrap().status,
        SandboxStateStatus::Destroyed
    );
}

#[tokio::test]
async fn refuses_to_destroy_while_a_tagged_instance_is_still_alive() {
    let h = Harness::new(live()).await;
    h.engine.apply_sandbox(&input()).await.unwrap();
    let b = h.begin(sbx_begin()).await.unwrap();
    h.engine.stop(&b.id, None).await.unwrap();
    let stuck = h.compute.launch_experiment(&b.id, &b.agent(Codex));
    h.compute.stubborn.lock().unwrap().insert(stuck.id.clone());
    let _ = h.engine.teardown(&b.id).await; // leaves the orphan
    let e = h.engine.destroy_sandbox(SBX).await.unwrap_err();
    assert_eq!(e.code, C::Orphans);
    assert!(e.message.contains(&stuck.id));
    assert_eq!(
        h.engine.sandbox_state(SBX).await.unwrap().status,
        SandboxStateStatus::Applied
    );
    h.compute.stubborn.lock().unwrap().clear();
    h.engine.teardown(&b.id).await.unwrap();
    h.engine.destroy_sandbox(SBX).await.unwrap();
    assert_eq!(
        h.engine.sandbox_state(SBX).await.unwrap().status,
        SandboxStateStatus::Destroyed
    );
}

#[tokio::test]
async fn fails_closed_when_the_orphan_check_itself_cannot_run() {
    let h = Harness::new(live()).await;
    h.engine.apply_sandbox(&input()).await.unwrap();
    *h.compute.fail_list_tagged.lock().unwrap() = Some(PortError::failed("ec2 throttled"));
    let e = h.engine.destroy_sandbox(SBX).await.unwrap_err();
    assert!(e.message.contains("ec2 throttled"), "{}", e.message);
    assert_eq!(
        h.engine.sandbox_state(SBX).await.unwrap().status,
        SandboxStateStatus::Applied
    );
}

#[tokio::test]
async fn treats_a_compute_that_does_not_exist_yet_no_sandbox_outputs_as_having_no_orphans() {
    let h = Harness::new(live()).await;
    *h.sandbox.fail_apply.lock().unwrap() = Some(PortError::failed("boom"));
    let _ = h.engine.apply_sandbox(&input()).await;
    *h.compute.fail_list_tagged.lock().unwrap() = Some(PortError::failed(format!(
        "{NOT_CONFIGURED}: apply the sandbox first"
    )));
    *h.sandbox.fail_apply.lock().unwrap() = None;
    h.engine.destroy_sandbox(SBX).await.unwrap();
    assert_eq!(
        h.engine.sandbox_state(SBX).await.unwrap().status,
        SandboxStateStatus::Destroyed
    );
}
