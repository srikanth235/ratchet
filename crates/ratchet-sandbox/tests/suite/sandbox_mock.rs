//! Port of `packages/aws/src/sandbox/mock.test.ts`.

use std::sync::Arc;

use ratchet_core::ports::PortError;
use ratchet_core::ports::SandboxProvider;
use ratchet_core::ports::{PlanAction, SandboxInput};
use ratchet_sandbox::sandbox::tofu::SharedRoles;
use ratchet_sandbox::sandbox::{
    MockSandboxProvider, MockSandboxProviderOptions, SANDBOX_RESOURCES,
};

fn input() -> SandboxInput {
    SandboxInput {
        sandbox_id: "sim".into(),
        region: "us-east-1".into(),
        allowed_instance_types: vec!["c7i.large".into()],
        max_instances_per_agent: 4,
        max_instance_lifetime_hours: 2.0,
        tags: None,
        environment: None,
    }
}

#[tokio::test]
async fn plans_every_sandbox_resource_with_real_addresses() {
    let p = MockSandboxProvider::default();
    let r = p.plan(&input()).await.unwrap();
    assert_eq!(r.changes.len(), SANDBOX_RESOURCES.len());
    assert!(r.changes.iter().all(|c| c.action == PlanAction::Create));
    assert_eq!(
        r.changes
            .iter()
            .map(|c| c.address.as_str())
            .collect::<Vec<_>>(),
        SANDBOX_RESOURCES.iter().map(|x| x.0).collect::<Vec<_>>()
    );
    assert!(
        r.changes
            .iter()
            .any(|c| c.address == "aws_s3_bucket_policy.sealed_deny_agents")
    );
    assert_eq!(
        r.summary,
        format!(
            "Plan: {} to add, 0 to change, 0 to destroy.",
            SANDBOX_RESOURCES.len()
        )
    );
}

#[tokio::test]
async fn apply_produces_contract_outputs_outputs_and_re_plan_reflect_it() {
    let p = MockSandboxProvider::default();
    assert!(p.outputs("sim").await.unwrap().is_none());
    let o = p.apply(&input()).await.unwrap();
    assert_eq!(o.artifacts_bucket, "ratchet-sim-artifacts");
    assert_eq!(o.agent_role_path, "/ratchet/agent/");
    assert_eq!(
        (
            o.limits.max_instances_per_agent,
            o.limits.max_instance_lifetime_hours
        ),
        (4, 2.0)
    );
    assert_eq!(p.outputs("sim").await.unwrap().as_ref(), Some(&o));
    assert_eq!(p.apply(&input()).await.unwrap(), o); // idempotent
    assert!(
        p.plan(&input())
            .await
            .unwrap()
            .changes
            .iter()
            .all(|c| c.action == PlanAction::NoOp)
    );
}

#[tokio::test]
async fn destroy_honours_the_teardown_guard_like_the_real_provider() {
    let roles = SharedRoles::default();
    roles.set(vec!["r1".into()]);
    let p = MockSandboxProvider::new(MockSandboxProviderOptions {
        role_lister: Some(Arc::new(roles.clone())),
        ..Default::default()
    });
    p.apply(&input()).await.unwrap();
    assert!(matches!(
        p.destroy(&input()).await.unwrap_err(),
        PortError::TeardownBlocked(_)
    ));
    assert!(p.outputs("sim").await.unwrap().is_some());
    roles.set(vec![]);
    p.destroy(&input()).await.unwrap();
    assert!(p.outputs("sim").await.unwrap().is_none());
}

#[tokio::test]
async fn can_start_with_sandboxes_that_are_already_applied() {
    let first = MockSandboxProvider::default();
    let out = first.apply(&input()).await.unwrap();
    let restored = MockSandboxProvider::new(MockSandboxProviderOptions {
        initial: [("sim".to_string(), out.clone())].into(),
        ..Default::default()
    });
    assert_eq!(restored.outputs("sim").await.unwrap(), Some(out));
    assert!(
        restored
            .plan(&input())
            .await
            .unwrap()
            .changes
            .iter()
            .all(|c| c.action == PlanAction::NoOp)
    );
}

#[tokio::test]
async fn validates_the_sandbox_id() {
    let mut bad = input();
    bad.sandbox_id = "BAD".into();
    assert!(MockSandboxProvider::default().plan(&bad).await.is_err());
}

#[tokio::test]
async fn records_the_calls_it_receives_in_order() {
    let p = MockSandboxProvider::default();
    p.plan(&input()).await.unwrap();
    p.apply(&input()).await.unwrap();
    p.destroy(&input()).await.unwrap();
    assert_eq!(p.calls(), ["plan:sim", "apply:sim", "destroy:sim"]);
}

#[tokio::test]
async fn is_usable_as_the_core_sandbox_provider_port_object_and_its_outputs_are_the_core_type() {
    let port: Arc<dyn SandboxProvider> = Arc::new(MockSandboxProvider::default());
    let out = port.apply(&input()).await.unwrap();
    assert_eq!(out.region, "us-east-1");
    assert!(out.deps_identity.is_some());
    // the outputs are core's: they survive the ledger's JSON column unchanged
    let json = serde_json::to_value(&out).unwrap();
    assert_eq!(
        serde_json::from_value::<ratchet_core::ports::SandboxOutputs>(json).unwrap(),
        out
    );
    assert_eq!(port.outputs("sim").await.unwrap(), Some(out));
}
