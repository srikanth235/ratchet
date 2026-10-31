//! `packages/engine/src/sandbox-auto.test.ts`

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ratchet_core::config::parse_config;
use ratchet_core::enums::{AgentsWhere, Mode};
use ratchet_core::ports::{
    EngineEvent as E, GitHost as _, PortError, SandboxInput, SandboxPhase, SandboxStateStatus,
    TeardownTrigger,
};
use ratchet_engine::sandbox_auto::region_of;
use ratchet_engine::{EngineErrorCode as C, EngineSettings, SandboxSettings};

use crate::common::*;

const MIN: i64 = 60_000;
const SBX: &str = "ratchet-sandbox"; // live mode, default region

fn live_with(sandbox: SandboxSettings, mut over: HarnessOptions) -> HarnessOptions {
    over.mode = Some(Mode::Live);
    over.settings = EngineSettings {
        sandbox,
        ..over.settings
    };
    over
}

fn auto() -> SandboxSettings {
    SandboxSettings {
        auto: Some(true),
        ..Default::default()
    }
}

async fn live() -> Harness {
    Harness::new(live_with(auto(), Default::default())).await
}

/// Create + start another optimization (the harness's own `begin` always uses the id `opt-test`).
async fn run(h: &Harness, id: &str, agents: &[AgentKind], wher: Option<AgentsWhere>) -> String {
    h.bench.set(
        "main-sha",
        "baseline",
        ScriptedBench::samples(&[100.0, 102.0, 98.0]).with_metric("peak_rss", 500.0),
    );
    h.bench.set(
        "main-sha",
        "confirm",
        ScriptedBench::samples(&[100.0, 100.0, 100.0]),
    );
    h.engine
        .create_optimization(CreateOptimizationInput {
            id: Some(id.into()),
            repo_url: h.git.remote_url(),
            base_ref: "main".into(),
            agents: agents.to_vec(),
            budget_usd: 100.0,
            time_limit_seconds: 8 * 3600,
            plateau_evals: 50,
            agents_where: wher,
            ..Default::default()
        })
        .await
        .unwrap();
    h.engine.start(id).await.unwrap();
    id.to_owned()
}

async fn run_codex(h: &Harness, id: &str) -> String {
    run(h, id, &[Codex], None).await
}

fn sandbox_phases(h: &mut Harness) -> Vec<SandboxPhase> {
    h.events()
        .into_iter()
        .filter_map(|e| match e {
            E::Sandbox { phase, .. } => Some(phase),
            _ => None,
        })
        .collect()
}

fn applies(h: &Harness) -> Vec<String> {
    h.log.starting_with("sandbox.apply")
}

fn destroys(h: &Harness) -> Vec<String> {
    h.log.starting_with("sandbox.destroy")
}

/// Advance the clock and tick (the engine's `run()` loop does this every tickIntervalMs).
async fn pass(h: &Harness, ms: i64) {
    h.clock.advance(ms);
    h.engine.tick().await.unwrap();
    h.engine.settle().await;
}

fn quiet() -> Option<ratchet_engine::settings::OnError> {
    Some(Arc::new(|_, _| {}))
}

fn draft_input(h: &Harness) -> CreateOptimizationInput {
    CreateOptimizationInput {
        repo_url: h.git.remote_url(),
        base_ref: "main".into(),
        agents: vec![Codex],
        budget_usd: 10.0,
        time_limit_seconds: 600,
        plateau_evals: 3,
        ..Default::default()
    }
}

fn explicit_input() -> SandboxInput {
    SandboxInput {
        sandbox_id: SBX.into(),
        region: "us-east-1".into(),
        allowed_instance_types: vec!["c7i.xlarge".into()],
        max_instances_per_agent: 2,
        max_instance_lifetime_hours: 4.0,
        tags: None,
        environment: None,
    }
}

async fn sandbox_id_of(h: &Harness, id: &str) -> Option<String> {
    h.ledger
        .get_optimization(&oid(id))
        .await
        .unwrap()
        .unwrap()
        .sandbox_id
        .map(|s| s.to_string())
}

async fn status_of(h: &Harness, sbx: &str) -> SandboxStateStatus {
    h.engine.sandbox_state(sbx).await.unwrap().status
}

// ------------------------------------------------------------------------------------------------ regionOf

#[test]
fn region_of_reads_environment_region_defensively_and_falls_back_to_the_setting_default() {
    let mut cfg = parse_config(CONFIG_YAML).expect("config");
    let set = |cfg: &mut ratchet_core::config::RatchetConfig, region: Option<&str>| {
        cfg.environment = Some(ratchet_core::config::Environment {
            scope: None,
            image: None,
            machine: None,
            region: region.map(str::to_owned),
            network: None,
        });
    };
    set(&mut cfg, Some("eu-west-1"));
    assert_eq!(region_of(&cfg, "us-east-1"), "eu-west-1");
    set(&mut cfg, Some("  "));
    assert_eq!(region_of(&cfg, "us-east-1"), "us-east-1");
    set(&mut cfg, None);
    assert_eq!(region_of(&cfg, "us-east-1"), "us-east-1");
    cfg.environment = None;
    assert_eq!(region_of(&cfg, "us-east-2"), "us-east-2");
    // `region: 42` (a non-string) cannot be expressed in the typed config: it is refused when parsed
}

// ------------------------------------------------------------------------------------ automatic sandbox: creation

#[tokio::test]
async fn is_created_at_the_first_run_start_not_before_from_ratchet_yaml_and_the_default_region() {
    let mut h = live().await;
    let id = h.engine.create_optimization(draft_input(&h)).await.unwrap();
    assert!(applies(&h).is_empty()); // a draft needs no environment
    assert_eq!(sandbox_id_of(&h, &id).await, None);

    h.bench.set(
        "main-sha",
        "baseline",
        ScriptedBench::samples(&[100.0, 100.0, 100.0]),
    );
    h.bench.set(
        "main-sha",
        "confirm",
        ScriptedBench::samples(&[100.0, 100.0, 100.0]),
    );
    let seen: Arc<Mutex<Option<(String, Vec<String>)>>> = Arc::default();
    let s2 = seen.clone();
    *h.sandbox.on_apply.lock().unwrap() = Some(Arc::new(move |i: &SandboxInput| {
        *s2.lock().unwrap() = Some((i.region.clone(), i.allowed_instance_types.clone()));
    }));
    h.engine.start(&id).await.unwrap();
    assert_eq!(applies(&h), [format!("sandbox.apply:{SBX}")]);
    // agents' type + the evaluator's hardware
    assert_eq!(
        seen.lock().unwrap().clone(),
        Some((
            "us-east-1".to_owned(),
            vec!["c7i.xlarge".to_owned(), "c7i.2xlarge".to_owned()]
        ))
    );
    let o = h.ledger.get_optimization(&oid(&id)).await.unwrap().unwrap();
    assert_eq!(o.sandbox_id.as_ref().map(|s| s.as_str()), Some(SBX));
    assert_eq!(o.status, OptimizationStatus::Running);
    let st = h.engine.sandbox_state(SBX).await.unwrap();
    assert_eq!(st.status, SandboxStateStatus::Applied);
    assert_eq!(st.region.as_deref(), Some("us-east-1"));
    assert_eq!(
        sandbox_phases(&mut h),
        [SandboxPhase::Creating, SandboxPhase::Ready]
    );
}

#[tokio::test]
async fn uses_the_configured_default_region_and_id_scheme_other_regions_get_their_own_sandbox() {
    let h = Harness::new(live_with(
        SandboxSettings {
            auto: Some(true),
            default_region: Some("eu-west-1".into()),
            ..Default::default()
        },
        Default::default(),
    ))
    .await;
    run_codex(&h, "run-a").await;
    assert_eq!(sandbox_id_of(&h, "run-a").await.as_deref(), Some(SBX));
    assert_eq!(
        h.ledger.get_sandbox(SBX).await.unwrap().unwrap().region,
        "eu-west-1"
    );

    let named = Harness::new(live_with(
        SandboxSettings {
            auto: Some(true),
            sandbox_id: Some(Arc::new(|r| format!("box-{r}"))),
            input: Some(Arc::new(|c| {
                Some(SandboxInput {
                    sandbox_id: c.sandbox_id.to_owned(),
                    region: c.region.to_owned(),
                    allowed_instance_types: vec!["m7i.large".into()],
                    max_instances_per_agent: 1,
                    max_instance_lifetime_hours: 1.0,
                    tags: None,
                    environment: None,
                })
            })),
            ..Default::default()
        },
        Default::default(),
    ))
    .await;
    run_codex(&named, "run-b").await;
    assert_eq!(
        sandbox_id_of(&named, "run-b").await.as_deref(),
        Some("box-us-east-1")
    );
    let types = named.sandbox.applies.lock().unwrap()[0]
        .allowed_instance_types
        .clone();
    assert_eq!(types, ["m7i.large"]);
}

#[tokio::test]
async fn simulation_ids_differ_from_live_ones() {
    let h = Harness::new(HarnessOptions {
        mode: Some(Mode::Simulation),
        settings: EngineSettings {
            sandbox: auto(),
            ..Default::default()
        },
        ..Default::default()
    })
    .await;
    run_codex(&h, "run-a").await;
    assert_eq!(
        sandbox_id_of(&h, "run-a").await.as_deref(),
        Some("ratchet-sim")
    );
}

#[tokio::test]
async fn is_reused_while_runs_are_active_concurrent_first_starts_apply_once() {
    let h = live().await;
    tokio::join!(
        run_codex(&h, "run-a"),
        run_codex(&h, "run-b"),
        run_codex(&h, "run-c")
    );
    assert_eq!(applies(&h).len(), 1);
    for i in ["run-a", "run-b", "run-c"] {
        assert_eq!(sandbox_id_of(&h, i).await.as_deref(), Some(SBX));
    }
    run_codex(&h, "run-d").await;
    assert_eq!(applies(&h).len(), 1);
}

#[tokio::test]
async fn a_failed_creation_leaves_the_optimization_a_draft_with_a_clear_error_and_a_retry_works() {
    let mut h = live().await;
    *h.sandbox.fail_apply.lock().unwrap() = Some(PortError::failed("tofu apply failed (exit 1)"));
    h.bench.set(
        "main-sha",
        "baseline",
        ScriptedBench::samples(&[100.0, 100.0, 100.0]),
    );
    h.bench.set(
        "main-sha",
        "confirm",
        ScriptedBench::samples(&[100.0, 100.0, 100.0]),
    );
    let id = h.engine.create_optimization(draft_input(&h)).await.unwrap();
    let err = h.engine.start(&id).await.unwrap_err();
    assert_eq!(err.code, C::SandboxUnavailable);
    assert!(
        err.message
            .contains("could not create the environment in us-east-1: tofu apply failed"),
        "{}",
        err.message
    );
    assert_eq!(h.status(&id).await, OptimizationStatus::Draft);
    assert!(!h.engine.run_active());
    assert_eq!(
        sandbox_phases(&mut h),
        [SandboxPhase::Creating, SandboxPhase::Failed]
    );
    *h.sandbox.fail_apply.lock().unwrap() = None;
    h.engine.start(&id).await.unwrap();
    assert_eq!(h.status(&id).await, OptimizationStatus::Running);
}

#[tokio::test]
async fn an_auto_sandbox_is_off_unless_asked_for_the_explicit_rules_still_apply() {
    let h = Harness::new(HarnessOptions {
        mode: Some(Mode::Live),
        ..Default::default()
    })
    .await;
    let e = h
        .engine
        .create_optimization(draft_input(&h))
        .await
        .unwrap_err();
    assert_eq!(e.code, C::SandboxNotApplied);
}

// ----------------------------------------------------------------------- automatic sandbox: idle window and teardown

#[tokio::test]
async fn is_not_destroyed_while_a_run_is_active_however_long_it_takes() {
    let h = live().await;
    run_codex(&h, "run-a").await;
    pass(&h, 5 * 60 * MIN).await;
    assert!(destroys(&h).is_empty());
    assert_eq!(status_of(&h, SBX).await, SandboxStateStatus::Applied);
    let act = h.engine.sandbox_activity(SBX).await.unwrap();
    assert!(act.auto);
    assert_eq!(act.active_runs, [oid("run-a")]);
    assert_eq!(act.idle_since, None);
    assert_eq!(act.destroy_at, None);
    assert_eq!(act.last_receipt, None);
}

#[tokio::test]
async fn is_destroyed_after_the_last_run_ends_plus_the_idle_window_default_30_min_with_the_ended_runs_torn_down_first()
 {
    let mut h = live().await;
    run(&h, "run-a", &[ClaudeCode, Codex], Some(AgentsWhere::Cloud)).await;
    pass(&h, 10 * MIN).await; // some runner time to bill
    h.engine.stop("run-a", None).await.unwrap();
    assert_eq!(h.compute.identities.lock().unwrap().len(), 2);

    pass(&h, 29 * MIN).await;
    assert!(destroys(&h).is_empty()); // still inside the window
    let waiting = h.engine.sandbox_activity(SBX).await.unwrap();
    assert!(waiting.active_runs.is_empty());
    assert_eq!(
        waiting.destroy_at,
        Some(waiting.idle_since.unwrap() + 30 * MIN)
    );
    assert_eq!(h.engine.teardown_receipt("run-a").await.unwrap(), None);

    pass(&h, 2 * MIN).await;
    assert_eq!(destroys(&h), [format!("sandbox.destroy:{SBX}")]);
    // the ended run's cloud resources go first, then the sandbox itself
    let log = h.log.all();
    let destroy_at = log
        .iter()
        .position(|l| l == &format!("sandbox.destroy:{SBX}"))
        .unwrap();
    let last_delete = log
        .iter()
        .rposition(|l| l == "compute.deleteIdentity:run-a-claude-code")
        .unwrap();
    assert!(destroy_at > last_delete);
    assert_eq!(h.status("run-a").await, OptimizationStatus::Completed); // the engine's own teardown ran
    assert!(
        h.compute
            .identities
            .lock()
            .unwrap()
            .values()
            .all(|i| i.deleted)
    );
    assert!(h.sandbox.applied.lock().unwrap().is_empty());
    assert!(h.compute.live(None).is_empty());
    let st = h.engine.sandbox_state(SBX).await.unwrap();
    assert_eq!(st.status, SandboxStateStatus::Destroyed);
    assert!(st.outputs.is_none());
    assert_eq!(
        sandbox_phases(&mut h),
        [
            SandboxPhase::Creating,
            SandboxPhase::Ready,
            SandboxPhase::Idle,
            SandboxPhase::Destroying,
            SandboxPhase::Destroyed
        ]
    );
}

#[tokio::test]
async fn records_a_receipt_what_was_removed_what_it_cost_nothing_left_running() {
    let h = Harness::new(live_with(
        auto(),
        HarnessOptions {
            hourly: Some(2.0),
            ..Default::default()
        },
    ))
    .await;
    run(&h, "run-a", &[Codex], Some(AgentsWhere::Cloud)).await;
    pass(&h, 60 * MIN).await;
    h.engine.stop("run-a", None).await.unwrap();
    let spend = h.ledger.total_spend(&oid("run-a")).await.unwrap();
    assert!(spend > 1.0, "{spend}"); // an hour of a $2/h runner
    pass(&h, 31 * MIN).await;

    let receipt = h
        .engine
        .teardown_receipt("run-a")
        .await
        .unwrap()
        .expect("receipt");
    assert_eq!(receipt.sandbox_id, SBX);
    assert_eq!(receipt.region, "us-east-1");
    assert_eq!(receipt.trigger, TeardownTrigger::Idle);
    assert!(receipt.nothing_running);
    assert!(receipt.orphans.is_empty() && receipt.errors.is_empty());
    assert_eq!(receipt.runs.len(), 1);
    assert_eq!(receipt.runs[0].optimization_id, oid("run-a"));
    assert_eq!(receipt.runs[0].status, "completed");
    let total = h.ledger.total_spend(&oid("run-a")).await.unwrap();
    assert!((receipt.spend_usd - total).abs() < 1e-6);
    assert!(receipt.spend_usd >= spend);
    assert!(receipt.spend_by_category["agent_runner"] > 1.0);
    assert_eq!(receipt.removed.instances.len(), 1);
    assert_eq!(receipt.removed.agent_identities, ["run-a-codex"]);
    assert_eq!(receipt.removed.secrets, ["run-a-codex"]);
    // (the Rust fake provider names its buckets `ratchet-artifacts-<id>` / `ratchet-sealed-<id>`)
    let resources = receipt.removed.resources.join("\n");
    assert!(
        resources.contains("ratchet-artifacts-ratchet-sandbox") && resources.contains("sealed"),
        "{resources}"
    );
    assert_eq!(receipt.destroyed_at, h.clock.now());
    assert!(receipt.created_at.is_some());
    let re = receipt.summary.contains("removed after 1 run, $")
        && receipt.summary.contains("Nothing is left running");
    assert!(re, "{}", receipt.summary);
    assert_eq!(
        h.engine.sandbox_activity(SBX).await.unwrap().last_receipt,
        Some(receipt)
    );
}

#[tokio::test]
async fn honours_a_configured_idle_window() {
    let h = Harness::new(live_with(
        SandboxSettings {
            auto: Some(true),
            idle_ms: Some(5 * MIN),
            ..Default::default()
        },
        Default::default(),
    ))
    .await;
    run_codex(&h, "run-a").await;
    h.engine.stop("run-a", None).await.unwrap();
    pass(&h, 4 * MIN).await;
    assert!(destroys(&h).is_empty());
    pass(&h, 2 * MIN).await;
    assert_eq!(destroys(&h).len(), 1);
}

#[tokio::test]
async fn a_run_that_starts_inside_the_idle_window_reuses_the_sandbox_and_stops_the_clock() {
    let h = live().await;
    run_codex(&h, "run-a").await;
    h.engine.stop("run-a", None).await.unwrap();
    pass(&h, 25 * MIN).await;
    run_codex(&h, "run-b").await;
    assert_eq!(applies(&h).len(), 1);
    pass(&h, 60 * MIN).await;
    assert!(destroys(&h).is_empty());
    assert_eq!(
        h.engine.sandbox_activity(SBX).await.unwrap().active_runs,
        [oid("run-b")]
    );
    // the window restarts from the end of run-b, not from run-a
    h.engine.stop("run-b", None).await.unwrap();
    pass(&h, 29 * MIN).await;
    assert!(destroys(&h).is_empty());
    pass(&h, 2 * MIN).await;
    assert_eq!(destroys(&h).len(), 1);
    let receipt = h.engine.teardown_receipt("run-a").await.unwrap().unwrap();
    let mut runs: Vec<_> = receipt
        .runs
        .iter()
        .map(|r| r.optimization_id.to_string())
        .collect();
    runs.sort();
    assert_eq!(runs, ["run-a", "run-b"]);
    assert_eq!(
        h.engine.teardown_receipt("run-b").await.unwrap(),
        Some(receipt)
    );
}

#[tokio::test]
async fn a_run_after_the_teardown_creates_a_fresh_sandbox() {
    let h = live().await;
    run_codex(&h, "run-a").await;
    h.engine.stop("run-a", None).await.unwrap();
    pass(&h, 31 * MIN).await;
    assert!(h.sandbox.applied.lock().unwrap().is_empty());
    run_codex(&h, "run-b").await;
    assert_eq!(applies(&h).len(), 2);
    assert_eq!(sandbox_id_of(&h, "run-b").await.as_deref(), Some(SBX));
    assert_eq!(status_of(&h, SBX).await, SandboxStateStatus::Applied);
    assert_eq!(h.engine.teardown_receipt("run-b").await.unwrap(), None); // its sandbox is alive again
}

#[tokio::test]
async fn keeps_the_idle_clock_across_an_app_restart_a_sandbox_that_outlived_the_app_goes_on_the_first_tick()
 {
    let mut h = live().await;
    run_codex(&h, "run-a").await;
    h.engine.stop("run-a", None).await.unwrap();
    pass(&h, 10 * MIN).await;
    h.restart(false).await;
    h.engine.ready().await;
    pass(&h, 10 * MIN).await;
    assert!(destroys(&h).is_empty()); // 20 min of idleness in total
    h.restart(false).await;
    pass(&h, 11 * MIN).await;
    assert_eq!(destroys(&h).len(), 1);
    // closed for a day: the first tick after launch removes it
    let mut g = live().await;
    run_codex(&g, "run-a").await;
    g.engine.stop("run-a", None).await.unwrap();
    g.clock.advance(24 * 60 * MIN);
    g.restart(false).await;
    g.engine.tick().await.unwrap();
    assert_eq!(destroys(&g).len(), 1);
}

#[tokio::test]
async fn a_blocked_destroy_an_instance_that_will_not_terminate_is_reported_in_the_receipt_path_not_hidden_and_retried_later()
 {
    let h = Harness::new(live_with(
        SandboxSettings {
            auto: Some(true),
            retry_ms: Some(10 * MIN),
            ..Default::default()
        },
        HarnessOptions {
            settings: EngineSettings {
                on_error: quiet(),
                ..Default::default()
            },
            ..Default::default()
        },
    ))
    .await;
    run(&h, "run-a", &[Codex], Some(AgentsWhere::Cloud)).await;
    let runner = h.compute.live(Some("run-a"))[0].clone();
    h.compute.stubborn.lock().unwrap().insert(runner.id.clone());
    h.engine.stop("run-a", None).await.unwrap();
    pass(&h, 31 * MIN).await;
    assert!(destroys(&h).is_empty()); // the orphan check refused
    assert_eq!(status_of(&h, SBX).await, SandboxStateStatus::Applied);
    assert_eq!(h.engine.teardown_receipt("run-a").await.unwrap(), None);
    assert!(
        h.engine
            .errors()
            .iter()
            .any(|e| e.context.contains("auto destroy") || e.context.contains("teardown"))
    );

    h.compute.stubborn.lock().unwrap().remove(&runner.id);
    pass(&h, 2 * MIN).await;
    assert!(destroys(&h).is_empty()); // retry back-off
    pass(&h, 10 * MIN).await;
    assert_eq!(destroys(&h).len(), 1);
    assert!(
        h.engine
            .teardown_receipt("run-a")
            .await
            .unwrap()
            .unwrap()
            .nothing_running
    );
}

#[tokio::test]
async fn a_provider_failure_during_the_destroy_leaves_the_sandbox_failed_infrastructure_of_unknown_state_and_is_retried()
 {
    let mut h = Harness::new(live_with(
        SandboxSettings {
            auto: Some(true),
            retry_ms: Some(5 * MIN),
            ..Default::default()
        },
        HarnessOptions {
            settings: EngineSettings {
                on_error: quiet(),
                ..Default::default()
            },
            ..Default::default()
        },
    ))
    .await;
    run_codex(&h, "run-a").await;
    h.engine.stop("run-a", None).await.unwrap();
    *h.sandbox.fail_destroy.lock().unwrap() = Some(PortError::failed("tofu destroy failed"));
    pass(&h, 31 * MIN).await;
    assert_eq!(status_of(&h, SBX).await, SandboxStateStatus::Failed);
    assert_eq!(sandbox_phases(&mut h).last(), Some(&SandboxPhase::Failed));
    assert_eq!(h.engine.teardown_receipt("run-a").await.unwrap(), None);
    *h.sandbox.fail_destroy.lock().unwrap() = None;
    pass(&h, 6 * MIN).await;
    assert_eq!(destroys(&h).len(), 2);
    assert_eq!(status_of(&h, SBX).await, SandboxStateStatus::Destroyed);
    assert!(
        h.engine
            .teardown_receipt("run-a")
            .await
            .unwrap()
            .unwrap()
            .nothing_running
    );
}

#[tokio::test]
async fn an_explicitly_applied_sandbox_is_never_destroyed_automatically() {
    let h = live().await;
    h.engine.apply_sandbox(&explicit_input()).await.unwrap();
    run_codex(&h, "run-a").await;
    assert_eq!(applies(&h).len(), 1); // reused, not re-applied
    h.engine.stop("run-a", None).await.unwrap();
    pass(&h, 120 * MIN).await;
    assert!(destroys(&h).is_empty());
    assert!(!h.engine.sandbox_activity(SBX).await.unwrap().auto);
}

#[tokio::test]
async fn an_explicit_apply_or_destroy_takes_an_auto_sandbox_out_of_automatic_management() {
    let h = live().await;
    run_codex(&h, "run-a").await;
    h.engine.stop("run-a", None).await.unwrap();
    h.engine.teardown("run-a").await.unwrap();
    h.engine.apply_sandbox(&explicit_input()).await.unwrap();
    pass(&h, 120 * MIN).await;
    assert!(destroys(&h).is_empty());
    h.engine.destroy_sandbox(SBX).await.unwrap();
    assert_eq!(destroys(&h).len(), 1);
}

#[tokio::test]
async fn is_off_by_default_nothing_is_destroyed_behind_an_explicit_user() {
    let h = Harness::new(HarnessOptions {
        mode: Some(Mode::Live),
        ..Default::default()
    })
    .await;
    h.engine.apply_sandbox(&explicit_input()).await.unwrap();
    let b = h
        .begin(BeginOptions {
            sandbox_id: Some(SBX.into()),
            ..Default::default()
        })
        .await
        .unwrap();
    h.engine.stop(&b.id, None).await.unwrap();
    pass(&h, 600 * MIN).await;
    assert!(destroys(&h).is_empty());
}

// ------------------------------------------------------------------------------------------ endSandbox (manual)

#[tokio::test]
async fn refuses_while_a_run_is_active_then_tears_down_on_demand_with_a_receipt() {
    let h = live().await;
    run_codex(&h, "run-a").await;
    assert_eq!(
        h.engine.end_sandbox(SBX).await.unwrap_err().code,
        C::SandboxInUse
    );
    h.engine.stop("run-a", None).await.unwrap();
    let receipt = h.engine.end_sandbox(SBX).await.unwrap();
    assert_eq!(receipt.trigger, TeardownTrigger::Manual);
    assert!(receipt.nothing_running);
    assert_eq!(destroys(&h).len(), 1); // no waiting for the idle window
    assert_eq!(
        h.engine.teardown_receipt("run-a").await.unwrap(),
        Some(receipt)
    );
    assert_eq!(
        h.engine.end_sandbox(SBX).await.unwrap_err().code,
        C::InvalidState
    );
    assert_eq!(
        h.engine.end_sandbox("nope").await.unwrap_err().code,
        C::NotFound
    );
}

// ------------------------------------------------------------------------------ run-active (sleep prevention)

fn flips(h: &mut Harness) -> Vec<(bool, Vec<String>)> {
    h.events()
        .into_iter()
        .filter_map(|e| match e {
            E::RunActive {
                active,
                optimization_ids,
            } => Some((
                active,
                optimization_ids.iter().map(ToString::to_string).collect(),
            )),
            _ => None,
        })
        .collect()
}

#[tokio::test]
async fn flips_true_when_a_run_starts_even_while_its_environment_is_still_being_created_and_false_when_the_last_one_ends()
 {
    let mut h = live().await;
    assert!(!h.engine.run_active());
    let during = Arc::new(AtomicBool::new(false));
    let (d2, e2) = (during.clone(), h.engine.clone());
    *h.sandbox.on_apply.lock().unwrap() = Some(Arc::new(move |_| {
        d2.store(e2.run_active(), Ordering::SeqCst)
    }));
    run_codex(&h, "run-a").await;
    assert!(during.load(Ordering::SeqCst));
    assert!(h.engine.run_active());
    run_codex(&h, "run-b").await;
    assert_eq!(flips(&mut h), [(true, vec!["run-a".to_owned()])]); // only transitions are emitted
    h.engine.stop("run-a", None).await.unwrap();
    assert!(h.engine.run_active());
    h.engine.stop("run-b", None).await.unwrap();
    assert!(!h.engine.run_active());
    let actives: Vec<_> = flips(&mut h).into_iter().map(|(a, _)| a).collect();
    assert_eq!(actives, [true, false]);
}

#[tokio::test]
async fn works_without_the_automatic_sandbox_and_a_failed_start_does_not_leave_it_stuck() {
    let h = Harness::new(Default::default()).await;
    let b = begun(&h).await;
    assert!(h.engine.run_active());
    h.engine.stop(&b.id, None).await.unwrap();
    assert!(!h.engine.run_active());

    let bad = Harness::new(Default::default()).await;
    let id2 = bad
        .engine
        .create_optimization(draft_input(&bad))
        .await
        .unwrap();
    let err = bad.engine.start(&id2).await.unwrap_err(); // the baseline blows up
    assert!(
        err.message.contains("no scripted bench result"),
        "{}",
        err.message
    );
    assert!(!bad.engine.run_active());
}

#[tokio::test]
async fn is_true_again_after_a_restart_with_a_run_in_flight() {
    let mut h = Harness::new(Default::default()).await;
    begun(&h).await;
    h.restart(false).await;
    h.engine.ready().await;
    assert!(h.engine.run_active());
}

// ------------------------------------------------------------------------------------------ TICK constant sanity

#[test]
fn one_engine_tick_is_a_minute_in_the_harness_the_windows_above_are_multiples_of_it() {
    assert_eq!(TICK_MS, MIN);
}
