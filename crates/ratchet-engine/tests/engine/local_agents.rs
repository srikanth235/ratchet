//! `packages/engine/src/local-agents.test.ts`

use std::sync::Arc;

use ratchet_core::enums::AgentsWhere;
use ratchet_core::enums::Mode;
use ratchet_core::ports::{GitHost as _, RunnerHandle};
use ratchet_engine::{EngineErrorCode, EngineSettings, SandboxSettings};

use crate::common::*;

fn cloud_calls(log: &[String]) -> Vec<String> {
    log.iter()
        .filter(|l| {
            [
                "createAgentIdentity",
                "launchRunner",
                "revoke",
                "deleteIdentity",
                "deleteSecrets",
            ]
            .iter()
            .any(|p| l.starts_with(&format!("compute.{p}")))
                || l.starts_with("compute.put")
        })
        .cloned()
        .collect()
}

fn local() -> HarnessOptions {
    HarnessOptions {
        local_runners: true,
        ..Default::default()
    }
}

fn handle(opt: &str, agent: &str) -> RunnerHandle {
    RunnerHandle {
        optimization_id: oid(opt),
        agent_id: agent.into(),
        instance_id: format!("local:{agent}"),
    }
}

#[tokio::test]
async fn defaults_to_local_when_the_engine_has_a_local_runner_control_and_starts_agents_without_any_cloud_runner_resources()
 {
    // no model keys at all: local agents use the user's own CLI login
    let h = Harness::new(HarnessOptions {
        local_runners: true,
        settings: EngineSettings {
            secrets: Some(Arc::new(|_, _| None)),
            ..Default::default()
        },
        ..Default::default()
    })
    .await;
    let b = begun(&h).await;
    assert_eq!(h.engine.agents_where(&b.id).await, AgentsWhere::Local);
    assert!(h.runners.started.lock().unwrap().is_empty());
    let started = h.local_runners.started.lock().unwrap().clone();
    let handles: Vec<_> = started.iter().map(|(h, _)| h.clone()).collect();
    assert_eq!(
        handles,
        [
            handle(&b.id, &b.agent(ClaudeCode)),
            handle(&b.id, &b.agent(Codex))
        ]
    );
    let cfg = &started[0].1;
    assert_eq!(cfg.repo_url, h.git.remote_url());
    assert_eq!(cfg.base_ref, "main");
    assert_eq!(cfg.branch, "ratchet/opt-test/claude-code");
    assert_eq!(cfg.kind, ClaudeCode);
    assert!(cloud_calls(&h.log.all()).is_empty());
    assert_eq!(h.compute.instances.lock().unwrap().len(), 0);
    let agents: Vec<_> = h
        .ledger
        .list_agents(&oid(&b.id))
        .await
        .unwrap()
        .into_iter()
        .map(|a| (a.status, a.runner_instance_id))
        .collect();
    assert_eq!(
        agents,
        [
            (
                AgentStatus::Running,
                Some(format!("local:{}", b.agent(ClaudeCode)))
            ),
            (
                AgentStatus::Running,
                Some(format!("local:{}", b.agent(Codex)))
            ),
        ]
    );
    // the agents' branches are still created on the git host
    let branches: Vec<_> = h.git.branches.lock().unwrap().keys().cloned().collect();
    assert_eq!(
        branches,
        ["ratchet/opt-test/claude-code", "ratchet/opt-test/codex"]
    );
}

#[tokio::test]
async fn routes_events_messages_pause_resume_and_stop_to_the_local_control() {
    let h = Harness::new(local()).await;
    let b = begun(&h).await;
    let cc = b.agent(ClaudeCode);
    h.local_runners
        .emit(&cc, vec![msg_event("hello from the laptop")]);
    h.cycle(1).await;
    assert!(!h.local_runners.poll_calls.lock().unwrap().is_empty());
    assert!(h.runners.poll_calls.lock().unwrap().is_empty());
    let evs = h
        .ledger
        .agent_events(&cc.as_str().into(), 0, 1000)
        .await
        .unwrap();
    assert!(evs.iter().any(|e| {
        e.payload
            .as_ref()
            .and_then(|p| p.get("text"))
            .and_then(|t| t.as_str())
            == Some("hello from the laptop")
    }));

    h.engine.message_agent(&cc, "try the cache").await.unwrap();
    h.engine.pause_agent(&cc).await.unwrap();
    h.engine.resume_agent(&cc).await.unwrap();
    let kinds: Vec<_> = h
        .local_runners
        .sent_to(&cc)
        .iter()
        .map(|m| {
            serde_json::to_value(m).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(kinds, ["user", "pause", "resume"]);
    assert!(h.runners.sent.lock().unwrap().is_empty());

    h.engine.stop(&b.id, None).await.unwrap();
    let mut stopped = h.local_runners.stopped.lock().unwrap().clone();
    stopped.sort();
    let mut all: Vec<_> = b.agents.values().cloned().collect();
    all.sort();
    assert_eq!(stopped, all);
    assert!(h.runners.stopped.lock().unwrap().is_empty());
    assert_eq!(h.status(&b.id).await, OptimizationStatus::Stopped);
}

#[tokio::test]
async fn teardown_releases_the_local_workspaces_and_leaves_cloud_side_secrets_alone() {
    let h = Harness::new(local()).await;
    let b = begun(&h).await;
    h.engine.stop(&b.id, None).await.unwrap();
    h.engine.teardown(&b.id).await.unwrap();
    assert_eq!(*h.local_runners.released.lock().unwrap(), [b.id.clone()]);
    assert!(h.runners.released.lock().unwrap().is_empty());
    assert!(
        !h.log
            .all()
            .iter()
            .any(|l| l.starts_with("compute.deleteSecrets"))
    );
    let r = h.engine.teardown_report(&b.id).await.unwrap().unwrap();
    assert!(
        r.deleted_secrets_for.is_empty()
            && r.deleted_identities_for.is_empty()
            && r.errors.is_empty()
    );
    assert_eq!(h.status(&b.id).await, OptimizationStatus::Completed);
}

#[tokio::test]
async fn a_lost_local_agent_the_app_restarted_is_reported_failed_and_the_run_can_end() {
    let h = Harness::new(local()).await;
    let b = h
        .begin(BeginOptions {
            agents: Some(vec![Codex]),
            ..Default::default()
        })
        .await
        .unwrap();
    let codex = b.agent(Codex);
    h.local_runners.emit(
        &codex,
        vec![
            ev(AgentEventType::Status)
                .with_text("local agent lost: the app was restarted while it was running")
                .with_data(serde_json::json!({ "state": "failed", "reason": "lost" })),
        ],
    );
    h.cycle(1).await;
    assert_eq!(
        h.ledger
            .get_agent(&codex.as_str().into())
            .await
            .unwrap()
            .unwrap()
            .status,
        AgentStatus::Failed
    );
    h.engine.stop(&b.id, None).await.unwrap();
}

#[tokio::test]
async fn an_optimization_can_ask_for_the_cloud_which_keeps_todays_path_identity_secrets_runner_instance()
 {
    let h = Harness::new(local()).await;
    let b = h
        .begin(BeginOptions {
            agents: Some(vec![Codex]),
            agents_where: Some(AgentsWhere::Cloud),
            ..Default::default()
        })
        .await
        .unwrap();
    let codex = b.agent(Codex);
    assert_eq!(h.engine.agents_where(&b.id).await, AgentsWhere::Cloud);
    assert!(h.local_runners.started.lock().unwrap().is_empty());
    assert_eq!(h.runners.started.lock().unwrap().len(), 1);
    let log = h.log.all();
    assert!(log.contains(&format!("compute.createAgentIdentity:{codex}")));
    assert!(log.contains(&format!("compute.launchRunner:{codex}")));
    assert_eq!(h.compute.live(Some(&b.id)).len(), 1);
    h.engine.stop(&b.id, None).await.unwrap();
    h.engine.teardown(&b.id).await.unwrap();
    // the control may have nothing to release; the engine still asks
    assert_eq!(*h.runners.released.lock().unwrap(), [b.id.clone()]);
    assert!(h.local_runners.released.lock().unwrap().is_empty());
}

#[tokio::test]
async fn the_engine_setting_picks_the_default_and_an_engine_without_a_local_control_stays_in_the_cloud()
 {
    let cloud_default = Harness::new(HarnessOptions {
        local_runners: true,
        settings: EngineSettings {
            agents_where: Some(AgentsWhere::Cloud),
            ..Default::default()
        },
        ..Default::default()
    })
    .await;
    let a = cloud_default
        .begin(BeginOptions {
            agents: Some(vec![Codex]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(
        cloud_default.engine.agents_where(&a.id).await,
        AgentsWhere::Cloud
    );

    let plain = Harness::new(Default::default()).await;
    let b = plain
        .begin(BeginOptions {
            agents: Some(vec![Codex]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(plain.engine.agents_where(&b.id).await, AgentsWhere::Cloud);
    assert_eq!(plain.runners.started.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn local_agents_are_refused_where_there_is_no_local_control() {
    let h = Harness::new(Default::default()).await;
    let e = h
        .begin(BeginOptions {
            agents_where: Some(AgentsWhere::Local),
            ..Default::default()
        })
        .await
        .unwrap_err();
    assert_eq!(e.code, EngineErrorCode::InvalidInput);
    assert!(
        e.message.contains("no local runner control"),
        "{}",
        e.message
    );
    assert!(h.ledger.list_optimizations(None).await.unwrap().is_empty());
    // an unknown value cannot be expressed in Rust: it is refused when parsed
    assert!(serde_json::from_str::<AgentsWhere>("\"moon\"").is_err());
}

#[tokio::test]
async fn survives_an_engine_restart_the_choice_is_remembered_per_optimization() {
    let mut h = Harness::new(local()).await;
    let b = h
        .begin(BeginOptions {
            agents: Some(vec![Codex]),
            agents_where: Some(AgentsWhere::Cloud),
            ..Default::default()
        })
        .await
        .unwrap();
    h.restart(false).await;
    h.engine.ready().await;
    assert_eq!(h.engine.agents_where(&b.id).await, AgentsWhere::Cloud);
    h.engine
        .message_agent(&b.agent(Codex), "still cloud")
        .await
        .unwrap();
    let kinds: Vec<_> = h
        .runners
        .sent_to(&b.agent(Codex))
        .iter()
        .map(|m| {
            serde_json::to_value(m).unwrap()["type"]
                .as_str()
                .unwrap()
                .to_owned()
        })
        .collect();
    assert_eq!(kinds, ["user"]);
}

#[tokio::test]
async fn works_with_the_automatic_sandbox_the_evaluator_is_cloud_even_when_the_agents_are_not() {
    let h = Harness::new(HarnessOptions {
        mode: Some(Mode::Live),
        local_runners: true,
        settings: EngineSettings {
            sandbox: SandboxSettings {
                auto: Some(true),
                ..Default::default()
            },
            ..Default::default()
        },
        ..Default::default()
    })
    .await;
    let b = h
        .begin(BeginOptions {
            agents: Some(vec![Codex]),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(h.engine.agents_where(&b.id).await, AgentsWhere::Local);
    let log = h.log.all();
    assert!(log.contains(&"sandbox.apply:ratchet-sandbox".to_owned()));
    assert!(log.iter().any(|l| l.starts_with("bench.baseline")));
    assert!(cloud_calls(&log).is_empty());
    h.engine.stop(&b.id, None).await.unwrap();
    h.clock.advance(31 * 60_000);
    h.engine.tick().await.unwrap();
    let receipt = h.engine.teardown_receipt(&b.id).await.unwrap().unwrap();
    assert!(receipt.nothing_running);
    assert!(
        receipt.removed.instances.is_empty()
            && receipt.removed.agent_identities.is_empty()
            && receipt.removed.secrets.is_empty()
    );
    assert_eq!(*h.local_runners.released.lock().unwrap(), [b.id.clone()]);
}
