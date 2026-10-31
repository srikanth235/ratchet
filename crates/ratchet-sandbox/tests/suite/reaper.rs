//! Port of `packages/aws/src/reaper.test.ts`.

use std::collections::HashSet;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ratchet_core::enums::InstanceRole;
use ratchet_core::ids::{AgentId, OptimizationId};
use ratchet_core::ports::CloudCompute;
use ratchet_core::ports::mock::ManualClock;
use ratchet_core::ports::{
    AgentIdentity, BenchLaunchPurpose, BenchLaunchSpec, InstanceInfo, InstanceState, PortResult,
    RemoteCommand, RemoteCommandResult, RunnerLaunchSpec, SecretRef,
};
use ratchet_sandbox::reaper::{ReapReason, Reaper, ReaperOptions, ReaperPolicy};
use ratchet_sandbox::{Error, MockCloudCompute};

const HOUR: i64 = 3_600_000;

#[derive(Default)]
struct SetPolicy {
    ended: Mutex<HashSet<String>>,
    stopped: Mutex<HashSet<String>>,
    all_ended: bool,
}

#[async_trait]
impl ReaperPolicy for SetPolicy {
    async fn is_optimization_ended(&self, id: &str) -> bool {
        self.all_ended || self.ended.lock().unwrap().contains(id)
    }
    async fn is_agent_stopped(&self, opt: &str, agent: &str) -> bool {
        self.stopped
            .lock()
            .unwrap()
            .contains(&format!("{opt}/{agent}"))
    }
}

/// Wraps a compute, counting `terminate` calls and optionally failing them (what the TS tests do by monkey-patching).
struct Spy {
    inner: Arc<MockCloudCompute>,
    terminate_calls: Mutex<usize>,
    fail_terminate: Option<&'static str>,
}

#[async_trait]
impl CloudCompute for Spy {
    async fn create_agent_identity(
        &self,
        o: &OptimizationId,
        a: &AgentId,
    ) -> PortResult<AgentIdentity> {
        self.inner
            .create_agent_identity(&OptimizationId::from(o), &AgentId::from(a))
            .await
    }
    async fn revoke_agent_identity(&self, i: &AgentIdentity) -> PortResult<()> {
        self.inner.revoke_agent_identity(i).await
    }
    async fn delete_agent_identity(&self, i: &AgentIdentity) -> PortResult<()> {
        self.inner.delete_agent_identity(i).await
    }
    async fn put_secret(
        &self,
        o: &OptimizationId,
        a: &AgentId,
        n: &str,
        v: &str,
    ) -> PortResult<SecretRef> {
        self.inner
            .put_secret(&OptimizationId::from(o), &AgentId::from(a), n, v)
            .await
    }
    async fn delete_secrets(&self, o: &OptimizationId, a: &AgentId) -> PortResult<()> {
        self.inner
            .delete_secrets(&OptimizationId::from(o), &AgentId::from(a))
            .await
    }
    async fn launch_runner(&self, s: &RunnerLaunchSpec) -> PortResult<InstanceInfo> {
        self.inner.launch_runner(s).await
    }
    async fn launch_bench_instance(&self, s: &BenchLaunchSpec) -> PortResult<InstanceInfo> {
        self.inner.launch_bench_instance(s).await
    }
    async fn terminate(&self, ids: &[String]) -> PortResult<()> {
        *self.terminate_calls.lock().unwrap() += 1;
        if let Some(msg) = self.fail_terminate {
            return Err(Error::aws(msg, msg).into());
        }
        self.inner.terminate(ids).await
    }
    async fn list_tagged(&self, o: Option<&OptimizationId>) -> PortResult<Vec<InstanceInfo>> {
        self.inner.list_tagged(o).await
    }
    async fn run_command(&self, i: &str, c: &RemoteCommand) -> PortResult<RemoteCommandResult> {
        self.inner.run_command(i, c).await
    }
}

struct Rig {
    clock: Arc<ManualClock>,
    cloud: Arc<MockCloudCompute>,
    spy: Arc<Spy>,
    policy: Arc<SetPolicy>,
    reaper: Reaper,
}

fn setup(quota: Option<usize>, all_ended: bool, fail_terminate: Option<&'static str>) -> Rig {
    let clock = Arc::new(ManualClock::new(100 * HOUR));
    let cloud = Arc::new(MockCloudCompute::with_clock(clock.clone()));
    let spy = Arc::new(Spy {
        inner: cloud.clone(),
        terminate_calls: Mutex::new(0),
        fail_terminate,
    });
    let policy = Arc::new(SetPolicy {
        all_ended,
        ..Default::default()
    });
    let mut opts = ReaperOptions::new(spy.clone(), policy.clone(), 2.0).clock(clock.clone());
    if let Some(q) = quota {
        opts = opts.max_instances_per_agent(q);
    }
    Rig {
        clock,
        cloud,
        spy,
        policy,
        reaper: Reaper::new(opts),
    }
}

fn default_rig() -> Rig {
    setup(None, false, None)
}

async fn states(cloud: &MockCloudCompute) -> std::collections::HashMap<String, InstanceState> {
    cloud
        .list_tagged(None)
        .await
        .unwrap()
        .into_iter()
        .map(|i| (i.id, i.state))
        .collect()
}

fn bench_spec(opt: &str) -> BenchLaunchSpec {
    BenchLaunchSpec {
        optimization_id: opt.into(),
        instance_type: "c7i.2xlarge".into(),
        image_ref: "a".into(),
        purpose: BenchLaunchPurpose::Bench,
    }
}

#[tokio::test]
async fn terminates_instances_past_max_lifetime_and_leaves_young_ones() {
    let r = default_rig();
    let old = r.cloud.simulate_experiment("o1", "a1", "c7i.large");
    r.clock.advance(2 * HOUR + 1);
    let young = r.cloud.simulate_experiment("o1", "a1", "c7i.large");
    let rep = r.reaper.reap(None).await.unwrap();
    assert_eq!(rep.terminated.len(), 1);
    let t = &rep.terminated[0];
    assert_eq!(
        (
            t.instance_id.as_str(),
            t.optimization_id.as_str(),
            t.agent_id.as_ref().map(AgentId::as_str),
            t.role,
            t.reason,
            t.age_ms
        ),
        (
            old.id.as_str(),
            "o1",
            Some("a1"),
            InstanceRole::Experiment,
            ReapReason::MaxLifetime,
            2 * HOUR + 1
        )
    );
    assert_eq!(rep.scanned, 2);
    let st = states(&r.cloud).await;
    assert_eq!(
        (st[&old.id], st[&young.id]),
        (InstanceState::Terminated, InstanceState::Running)
    );
}

#[tokio::test]
async fn exactly_at_the_limit_is_kept() {
    let r = default_rig();
    r.cloud.simulate_experiment("o1", "a1", "c7i.large");
    r.clock.advance(2 * HOUR);
    assert!(r.reaper.reap(None).await.unwrap().terminated.is_empty());
}

#[tokio::test]
async fn terminates_instances_of_stopped_agents_only_that_agent() {
    let r = default_rig();
    let mine = r.cloud.simulate_experiment("o1", "a1", "c7i.large");
    let other = r.cloud.simulate_experiment("o1", "a2", "c7i.large");
    r.policy.stopped.lock().unwrap().insert("o1/a1".into());
    let rep = r.reaper.reap(None).await.unwrap();
    assert_eq!(
        rep.terminated
            .iter()
            .map(|t| (t.instance_id.clone(), t.reason))
            .collect::<Vec<_>>(),
        [(mine.id, ReapReason::AgentStopped)]
    );
    assert_eq!(states(&r.cloud).await[&other.id], InstanceState::Running);
}

#[tokio::test]
async fn terminates_everything_of_an_ended_optimization_including_bench_no_agent() {
    let r = default_rig();
    let e = r.cloud.simulate_experiment("o1", "a1", "c7i.large");
    let b = r
        .cloud
        .launch_bench_instance(&bench_spec("o1"))
        .await
        .unwrap();
    let keep = r.cloud.simulate_experiment("o2", "a1", "c7i.large");
    r.policy.ended.lock().unwrap().insert("o1".into());
    let rep = r.reaper.reap(None).await.unwrap();
    let mut got: Vec<_> = rep
        .terminated
        .iter()
        .map(|t| t.instance_id.clone())
        .collect();
    got.sort();
    let mut want = vec![e.id, b.id];
    want.sort();
    assert_eq!(got, want);
    assert!(
        rep.terminated
            .iter()
            .all(|t| t.reason == ReapReason::OptimizationEnded)
    );
    assert_eq!(states(&r.cloud).await[&keep.id], InstanceState::Running);
}

#[tokio::test]
async fn optimization_ended_takes_precedence_over_agent_stopped_and_lifetime() {
    let r = default_rig();
    r.cloud.simulate_experiment("o1", "a1", "c7i.large");
    r.clock.advance(5 * HOUR);
    r.policy.ended.lock().unwrap().insert("o1".into());
    r.policy.stopped.lock().unwrap().insert("o1/a1".into());
    assert_eq!(
        r.reaper.reap(None).await.unwrap().terminated[0].reason,
        ReapReason::OptimizationEnded
    );
}

#[tokio::test]
async fn terminates_merely_stopped_instances_terminate_on_shutdown_missed() {
    let r = default_rig();
    let i = r.cloud.simulate_experiment("o1", "a1", "c7i.large");
    r.cloud.simulate_stopped(&i.id);
    assert_eq!(
        r.reaper
            .reap(None)
            .await
            .unwrap()
            .terminated
            .iter()
            .map(|t| t.reason)
            .collect::<Vec<_>>(),
        [ReapReason::StoppedInstance]
    );
}

#[tokio::test]
async fn enforces_max_instances_per_agent_keeps_oldest_kills_newest_reports_offender() {
    let r = setup(Some(2), false, None);
    let mut ids = vec![];
    for _ in 0..4 {
        ids.push(r.cloud.simulate_experiment("o1", "a1", "c7i.large").id);
        r.clock.advance(1000);
    }
    r.cloud.simulate_experiment("o1", "a2", "c7i.large"); // within quota
    let rep = r.reaper.reap(None).await.unwrap();
    assert_eq!(
        rep.terminated
            .iter()
            .map(|t| (t.instance_id.clone(), t.reason))
            .collect::<Vec<_>>(),
        [
            (ids[2].clone(), ReapReason::OverQuota),
            (ids[3].clone(), ReapReason::OverQuota)
        ]
    );
    assert_eq!(rep.over_quota_agents.len(), 1);
    let o = &rep.over_quota_agents[0];
    assert_eq!(
        (o.optimization_id.as_str(), o.agent_id.as_str(), o.running),
        ("o1", "a1", 4)
    );
}

#[tokio::test]
async fn quota_ignores_runners_and_bench_no_quota_configured_means_no_enforcement() {
    let r = setup(Some(1), false, None);
    let id = r
        .cloud
        .create_agent_identity(&OptimizationId::from("o1"), &AgentId::from("a1"))
        .await
        .unwrap();
    r.cloud
        .launch_runner(&RunnerLaunchSpec {
            optimization_id: "o1".into(),
            agent_id: "a1".into(),
            kind: ratchet_core::enums::AgentKind::Mock,
            instance_type: "c7i.large".into(),
            image_ref: "a".into(),
            identity: id,
            secrets: vec![],
            repo_url: "r".into(),
            branch: "b".into(),
        })
        .await
        .unwrap();
    r.cloud.simulate_experiment("o1", "a1", "c7i.large");
    assert!(r.reaper.reap(None).await.unwrap().terminated.is_empty());

    let r2 = default_rig();
    for _ in 0..10 {
        r2.cloud.simulate_experiment("o1", "a1", "c7i.large");
    }
    assert!(r2.reaper.reap(None).await.unwrap().terminated.is_empty());
}

#[tokio::test]
async fn is_idempotent_second_run_finds_nothing_and_does_not_call_terminate_again() {
    let r = default_rig();
    r.cloud.simulate_experiment("o1", "a1", "c7i.large");
    r.clock.advance(3 * HOUR);
    assert_eq!(r.reaper.reap(None).await.unwrap().terminated.len(), 1);
    assert!(r.reaper.reap(None).await.unwrap().terminated.is_empty());
    assert_eq!(*r.spy.terminate_calls.lock().unwrap(), 1);
}

#[tokio::test]
async fn skips_instances_already_shutting_down_scopes_to_one_optimization_when_asked() {
    let r = default_rig();
    let a = r.cloud.simulate_experiment("o1", "a1", "c7i.large");
    let b = r.cloud.simulate_experiment("o2", "a1", "c7i.large");
    r.clock.advance(3 * HOUR);
    let rep = r
        .reaper
        .reap(Some(&OptimizationId::from("o1")))
        .await
        .unwrap();
    assert_eq!(
        rep.terminated
            .iter()
            .map(|t| t.instance_id.clone())
            .collect::<Vec<_>>(),
        [a.id]
    );
    assert_eq!(states(&r.cloud).await[&b.id], InstanceState::Running);
}

#[tokio::test]
async fn records_terminate_failures_in_the_report_instead_of_throwing() {
    let r = setup(None, false, Some("UnauthorizedOperation"));
    r.cloud.simulate_experiment("o1", "a1", "c7i.large");
    r.clock.advance(3 * HOUR);
    let rep = r.reaper.reap(None).await.unwrap();
    assert!(rep.terminated.is_empty());
    assert_eq!(rep.errors, ["terminate failed: UnauthorizedOperation"]);
}

#[tokio::test]
async fn supports_async_predicates() {
    let r = setup(None, true, None);
    r.cloud.simulate_experiment("o1", "a1", "c7i.large");
    assert_eq!(r.reaper.reap(None).await.unwrap().terminated.len(), 1);
}

#[tokio::test]
async fn only_ever_touches_instances_the_compute_lists_as_tagged_and_never_terminated_ones() {
    // Safety: a terminated instance is skipped, not re-terminated; an empty sandbox terminates nothing.
    let r = default_rig();
    assert!(r.reaper.reap(None).await.unwrap().terminated.is_empty());
    assert_eq!(*r.spy.terminate_calls.lock().unwrap(), 0);
    let i = r.cloud.simulate_experiment("o1", "a1", "c7i.large");
    r.cloud.terminate(&[i.id]).await.unwrap();
    r.clock.advance(10 * HOUR);
    assert!(r.reaper.reap(None).await.unwrap().terminated.is_empty());
    assert_eq!(*r.spy.terminate_calls.lock().unwrap(), 0);
}
