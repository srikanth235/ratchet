//! Port of `packages/aws/src/compute/mock.test.ts`.

use std::sync::Arc;

use ratchet_core::enums::{AgentKind, InstanceRole};
use ratchet_core::ids::{AgentId, OptimizationId};
use ratchet_core::ports::CloudCompute;
use ratchet_core::ports::mock::ManualClock;
use ratchet_core::ports::{
    AgentIdentity, BenchLaunchPurpose, BenchLaunchSpec, InstanceState, RemoteCommand,
    RemoteCommandResult, RunnerLaunchSpec,
};
use ratchet_sandbox::compute::mock::command_handler;
use ratchet_sandbox::{MockCloudCompute, MockCloudComputeOptions};

fn mk(boot_delay_ms: i64) -> (Arc<ManualClock>, MockCloudCompute) {
    let clock = Arc::new(ManualClock::new(1000));
    let cloud = MockCloudCompute::new(MockCloudComputeOptions {
        boot_delay_ms,
        ..MockCloudComputeOptions::with_clock(clock.clone())
    });
    (clock, cloud)
}

fn spec(identity: &AgentIdentity) -> RunnerLaunchSpec {
    RunnerLaunchSpec {
        optimization_id: "o1".into(),
        agent_id: "a1".into(),
        kind: AgentKind::Mock,
        instance_type: "c7i.large".into(),
        image_ref: "ami-1".into(),
        identity: identity.clone(),
        secrets: vec![],
        repo_url: "x".into(),
        branch: "b".into(),
    }
}

fn bench(opt: &str, instance_type: &str) -> BenchLaunchSpec {
    BenchLaunchSpec {
        optimization_id: opt.into(),
        instance_type: instance_type.into(),
        image_ref: "a".into(),
        purpose: BenchLaunchPurpose::Bench,
    }
}

#[tokio::test]
async fn validates_ids_like_the_real_one() {
    let (_, cloud) = mk(0);
    assert!(
        cloud
            .create_agent_identity(&OptimizationId::from("O 1"), &AgentId::from("a"))
            .await
            .unwrap_err()
            .to_string()
            .contains("invalid")
    );
    assert!(
        cloud
            .put_secret(&OptimizationId::from("o1"), &AgentId::from("a/b"), "K", "v")
            .await
            .is_err()
    );
}

#[tokio::test]
async fn identity_lifecycle_create_is_idempotent_revoke_delete() {
    let (_, cloud) = mk(0);
    let id = cloud
        .create_agent_identity(&OptimizationId::from("o1"), &AgentId::from("a1"))
        .await
        .unwrap();
    assert!(id.role_ref.contains("/ratchet/agent/ratchet-agent-o1-a1"));
    assert_eq!(
        cloud
            .create_agent_identity(&OptimizationId::from("o1"), &AgentId::from("a1"))
            .await
            .unwrap(),
        id
    );
    assert!(!cloud.is_revoked(&id));
    cloud.revoke_agent_identity(&id).await.unwrap();
    assert!(cloud.is_revoked(&id));
    cloud.delete_agent_identity(&id).await.unwrap();
    cloud.delete_agent_identity(&id).await.unwrap(); // idempotent
    assert!(cloud.identities().is_empty());
    assert!(
        cloud
            .revoke_agent_identity(&id)
            .await
            .unwrap_err()
            .to_string()
            .contains("NoSuchEntity")
    );
}

#[tokio::test]
async fn secrets_live_under_the_agent_path_and_delete_secrets_is_scoped() {
    let (_, cloud) = mk(0);
    let r = cloud
        .put_secret(
            &OptimizationId::from("o1"),
            &AgentId::from("a1"),
            "KEY",
            "v1",
        )
        .await
        .unwrap();
    cloud
        .put_secret(
            &OptimizationId::from("o1"),
            &AgentId::from("a2"),
            "KEY",
            "v2",
        )
        .await
        .unwrap();
    assert_eq!(r.reference, "/ratchet/o1/a1/KEY");
    assert_eq!(
        cloud.secret_value("/ratchet/o1/a1/KEY").as_deref(),
        Some("v1")
    );
    cloud
        .delete_secrets(&OptimizationId::from("o1"), &AgentId::from("a1"))
        .await
        .unwrap();
    assert_eq!(cloud.secret_names(), ["/ratchet/o1/a2/KEY"]);
}

#[tokio::test]
async fn launch_requires_an_existing_identity_instances_boot_after_the_delay() {
    let (clock, cloud) = mk(5000);
    let id = cloud
        .create_agent_identity(&OptimizationId::from("o1"), &AgentId::from("a1"))
        .await
        .unwrap();
    cloud.delete_agent_identity(&id).await.unwrap();
    let e = cloud.launch_runner(&spec(&id)).await.unwrap_err();
    assert!(
        e.to_string().contains("Invalid IAM Instance Profile"),
        "{e}"
    );
    let id2 = cloud
        .create_agent_identity(&OptimizationId::from("o1"), &AgentId::from("a1"))
        .await
        .unwrap();
    let inst = cloud.launch_runner(&spec(&id2)).await.unwrap();
    assert_eq!(
        (
            inst.role,
            inst.agent_id.as_ref().map(AgentId::as_str),
            inst.state,
            inst.launched_at
        ),
        (
            InstanceRole::AgentRunner,
            Some("a1"),
            InstanceState::Pending,
            1000
        )
    );
    clock.advance(5000);
    assert_eq!(
        cloud
            .list_tagged(Some(&OptimizationId::from("o1")))
            .await
            .unwrap()[0]
            .state,
        InstanceState::Running
    );
}

#[tokio::test]
async fn list_tagged_filters_by_optimization_terminate_is_idempotent_and_stamps_terminated_at() {
    let (clock, cloud) = mk(0);
    let id = cloud
        .create_agent_identity(&OptimizationId::from("o1"), &AgentId::from("a1"))
        .await
        .unwrap();
    let r = cloud.launch_runner(&spec(&id)).await.unwrap();
    let b = cloud
        .launch_bench_instance(&bench("o2", "c7i.2xlarge"))
        .await
        .unwrap();
    assert_eq!(b.agent_id, None);
    assert_eq!(
        cloud
            .list_tagged(Some(&OptimizationId::from("o2")))
            .await
            .unwrap()
            .iter()
            .map(|i| i.id.clone())
            .collect::<Vec<_>>(),
        std::slice::from_ref(&b.id)
    );
    assert_eq!(cloud.list_tagged(None).await.unwrap().len(), 2);
    clock.advance(10_000);
    cloud
        .terminate(&[r.id.clone(), "i-unknown".into()])
        .await
        .unwrap();
    clock.advance(10_000);
    cloud.terminate(std::slice::from_ref(&r.id)).await.unwrap();
    let t = cloud
        .list_tagged(Some(&OptimizationId::from("o1")))
        .await
        .unwrap()
        .remove(0);
    assert_eq!(
        (t.state, t.terminated_at),
        (InstanceState::Terminated, Some(11_000))
    );
}

#[tokio::test]
async fn run_command_delegates_to_the_injected_handler_and_needs_a_running_instance() {
    let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
    let seen2 = seen.clone();
    let handler = command_handler(move |inst, c| {
        seen2
            .lock()
            .unwrap()
            .push(format!("{}:{}", inst.id, c.script));
        RemoteCommandResult {
            exit_code: 0,
            stdout: "ok".into(),
            stderr: String::new(),
        }
    });
    let clock = Arc::new(ManualClock::new(1000));
    let cloud = MockCloudCompute::new(MockCloudComputeOptions {
        command_handler: Some(handler),
        ..MockCloudComputeOptions::with_clock(clock)
    });
    let id = cloud
        .create_agent_identity(&OptimizationId::from("o1"), &AgentId::from("a1"))
        .await
        .unwrap();
    let r = cloud.launch_runner(&spec(&id)).await.unwrap();
    let res = cloud
        .run_command(&r.id, &RemoteCommand::new("echo hi"))
        .await
        .unwrap();
    assert_eq!(
        res,
        RemoteCommandResult {
            exit_code: 0,
            stdout: "ok".into(),
            stderr: String::new()
        }
    );
    assert_eq!(*seen.lock().unwrap(), [format!("{}:echo hi", r.id)]);
    cloud.terminate(std::slice::from_ref(&r.id)).await.unwrap();
    assert!(
        cloud
            .run_command(&r.id, &RemoteCommand::new("x"))
            .await
            .unwrap_err()
            .to_string()
            .contains("not running")
    );
    assert!(
        cloud
            .run_command("i-nope", &RemoteCommand::new("x"))
            .await
            .unwrap_err()
            .to_string()
            .contains("InvalidInstanceId")
    );
}

#[tokio::test]
async fn default_handler_succeeds_silently_helpers_simulate_experiments_shutdown_and_stopped() {
    let (clock, cloud) = mk(0);
    let e = cloud.simulate_experiment("o1", "a1", "c7i.xlarge");
    assert_eq!(e.role, InstanceRole::Experiment);
    assert_eq!(
        cloud
            .run_command(&e.id, &RemoteCommand::new("true"))
            .await
            .unwrap()
            .exit_code,
        0
    );
    assert_eq!(cloud.commands().len(), 1);
    clock.advance(1000);
    cloud.simulate_stopped(&e.id);
    let first = cloud.list_tagged(None).await.unwrap().remove(0);
    assert_eq!(
        (first.state, first.terminated_at),
        (InstanceState::Stopped, Some(2000))
    );
    let f = cloud.simulate_experiment("o1", "a1", "c7i.xlarge");
    cloud.simulate_shutdown(&f.id);
    assert_eq!(
        cloud
            .list_tagged(None)
            .await
            .unwrap()
            .iter()
            .find(|i| i.id == f.id)
            .unwrap()
            .state,
        InstanceState::Terminated
    );
}

#[tokio::test]
async fn mock_instance_ids_are_valid_instance_ids_so_the_real_validators_accept_them() {
    let (_, cloud) = mk(0);
    let i = cloud.simulate_experiment("o1", "a1", "c7i.large");
    ratchet_sandbox::util::assert_valid_instance_id(&i.id).unwrap();
    assert_eq!(i.id, "i-mock000000000001");
}

#[tokio::test]
async fn is_usable_as_the_core_cloud_compute_port_object() {
    let (_, cloud) = mk(0);
    let port: Arc<dyn CloudCompute> = Arc::new(cloud);
    let id = port
        .create_agent_identity(&OptimizationId::from("o1"), &AgentId::from("a1"))
        .await
        .unwrap();
    let launched = port.launch_runner(&spec(&id)).await.unwrap();
    assert_eq!(launched.optimization_id, "o1");
    assert_eq!(
        port.list_tagged(Some(&OptimizationId::from("o1")))
            .await
            .unwrap()
            .len(),
        1
    );
    assert!(
        port.list_tagged(Some(&OptimizationId::from("o2")))
            .await
            .unwrap()
            .is_empty()
    );
}
