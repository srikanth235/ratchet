//! The simulation service behind the trait: seeded data, scripted runs, the ratchet rule, stop rules, the
//! environment lifecycle, persistence. (The HTTP side is covered by `contract.rs`.)

use ratchet_core::enums::{AgentStatus, CandidateVerdict, OptimizationStatus};
use ratchet_core::ids::OptimizationId;
use ratchetd::api::*;
use ratchetd::service::{AppService, ServiceError};
use ratchetd::sim::{SimOptions, SimService};

fn demo_input(budget: f64, plateau: u32) -> CreateOptimizationInput {
    CreateOptimizationInput {
        name: None,
        repo_url: String::new(),
        base_ref: "main".into(),
        demo: Some(DemoId::Search),
        agents: vec![
            ratchet_core::AgentKind::ClaudeCode,
            ratchet_core::AgentKind::Codex,
        ],
        agents_where: None,
        budget_usd: budget,
        time_limit_seconds: 3600,
        plateau_evals: plateau,
        connections: None,
        environment_draft_id: None,
    }
}

async fn start_new(sim: &SimService, budget: f64, plateau: u32) -> String {
    let id = sim
        .create_optimization(demo_input(budget, plateau))
        .await
        .unwrap()
        .id;
    sim.start_optimization(&id).await.unwrap();
    id
}

#[tokio::test]
async fn the_seed_has_a_live_run_two_finished_ones_and_a_draft() {
    let sim = SimService::manual().await.unwrap();
    let snap = sim.list_optimizations(KpiRange::Days30).await.unwrap();
    let by_id = |id: &str| snap.optimizations.iter().find(|o| o.id == id).unwrap();
    assert_eq!(snap.optimizations.len(), 4);
    let live = by_id("opt_demo_search");
    assert_eq!(live.status, UiStatus::Running);
    assert_eq!(live.verified_wins, 5);
    assert_eq!(live.candidate_count, 11);
    assert_eq!((live.baseline, live.best), (Some(182.0), Some(131.0)));
    assert!((live.gain_pct.unwrap() - 28.02).abs() < 0.01);
    assert_eq!(live.agents.len(), 2);
    assert!(live.value.is_some(), "the search demo has a value model");
    assert_eq!(by_id("opt_demo_ingest").status, UiStatus::Completed);
    assert_eq!(by_id("opt_demo_edge").status, UiStatus::Completed);
    assert_eq!(by_id("opt_demo_web").status, UiStatus::Draft);
    assert_eq!(by_id("opt_demo_web").baseline, None);
    // KPIs: 30 days sees the search and ingest wins (5 + 2), not the six-week-old edge win
    let kpi = |id: &str| snap.kpis.iter().find(|k| k.id == id).unwrap();
    assert_eq!(kpi("wins").value, "7");
    assert_eq!(kpi("wins").unit.as_deref(), Some("last 30d"));
    assert!(kpi("value").value.starts_with('$'));
    assert_eq!(snap.kpis.len(), 5);
    let wide = sim.list_optimizations(KpiRange::Days90).await.unwrap();
    assert_eq!(
        wide.kpis.iter().find(|k| k.id == "wins").unwrap().value,
        "8"
    );
    assert_eq!(snap.activity.len(), 8);
    assert!(
        snap.activity[0].at_ms >= snap.activity[7].at_ms,
        "newest first"
    );
    // the engine says what it is
    assert_eq!(snap.engine.kind, EngineKind::Sim);
}

#[tokio::test]
async fn the_seeded_detail_has_everything_the_result_screen_needs() {
    let sim = SimService::manual().await.unwrap();
    let d = sim.get_optimization("opt_demo_search").await.unwrap();
    assert_eq!(d.agents.len(), 2);
    assert!(
        d.agents
            .iter()
            .all(|a| a.stats.candidates > 0 && a.hypothesis.is_some() && a.last_event_id > 0)
    );
    let keys: Vec<&str> = d.compare.iter().map(|r| r.key.as_str()).collect();
    assert_eq!(
        keys,
        [
            "http_req_duration.p50",
            "http_req_duration.p95",
            "http_req_duration.p99",
            "error_rate",
            "peak_rss_mb"
        ],
        "the objective family, then the guardrail metrics"
    );
    assert!(d.compare[1].delta_pct.unwrap() < -25.0);
    assert_eq!(d.compare[4].better, Better::Lower);
    let spans = d.spans.unwrap();
    assert_eq!(
        spans.iter().map(|s| s.name.as_str()).collect::<Vec<_>>(),
        ["parse", "search", "serialize", "wait"]
    );
    assert!(spans.iter().all(|s| s.best <= s.baseline + 1e-9));
    assert!(d.calibration.unwrap().contains("wins must beat"));
    assert!(d.baseline_samples.len() == 5 && d.noise_floor == Some(3.1));
    assert_eq!(d.pipeline.stages.len(), 5);
    assert!(d.pr_url.is_none());
    assert!(d.config_yaml.contains("search-api p95 latency"));
    // the last seeded candidate decided: every stage is done
    assert_eq!(d.pipeline.candidate.unwrap().sha.len(), 7);
}

#[tokio::test]
async fn candidates_series_and_spend_agree_with_the_ledger() {
    let sim = SimService::manual().await.unwrap();
    let c = sim.candidates("opt_demo_search").await.unwrap();
    let verified: Vec<&CandidateVm> = c.iter().filter(|c| c.moved_ratchet).collect();
    assert_eq!(verified.len(), 5);
    assert!(
        verified
            .iter()
            .all(|c| c.verdict == CandidateVerdict::Verified && c.confirm_value.is_some())
    );
    let failed = c
        .iter()
        .find(|c| c.verdict == CandidateVerdict::FailedConfirm)
        .unwrap();
    assert_eq!(failed.bench_value, Some(148.0));
    assert!(
        failed.vs_baseline_pct.unwrap() > 18.0,
        "it looked great on the bench"
    );
    assert!(!failed.moved_ratchet);
    let blocked = c
        .iter()
        .find(|c| c.verdict == CandidateVerdict::Blocked)
        .unwrap();
    assert!(blocked.reasons[0].contains("protected path"));

    let s = sim.ratchet_history("opt_demo_search", None).await.unwrap();
    assert_eq!(s.steps.len(), 6, "baseline plus five moves");
    assert_eq!(
        s.steps[0],
        StepPoint {
            t: 0.0,
            value: 182.0
        }
    );
    assert!(
        s.steps
            .windows(2)
            .all(|w| w[1].value < w[0].value && w[1].t > w[0].t),
        "the ratchet only turns one way"
    );
    assert_eq!(s.points.len(), 11);
    assert_eq!(s.points.iter().filter(|p| p.verified).count(), 5);
    assert!(s.t_max >= 214.0, "live: runs to now");
    assert_eq!(s.direction, Better::Lower);

    let sp = sim.spend("opt_demo_search").await.unwrap();
    assert!((sp.total - 42.6).abs() < 0.2);
    assert_eq!(sp.by_category[0].id, "tokens", "sorted by amount");
    assert_eq!(sp.by_agent.len(), 2);
    assert!(sp.series.windows(2).all(|w| w[1].usd >= w[0].usd));
    assert_eq!(sp.budget_usd, 120.0);
    assert!(matches!(
        sim.candidates("nope").await,
        Err(ServiceError::NotFound(_))
    ));
}

#[tokio::test]
async fn a_started_run_baselines_then_ticks_through_the_real_rule() {
    let sim = SimService::manual().await.unwrap();
    let mut events = sim.subscribe();
    let id = start_new(&sim, 100.0, 50).await;
    let d = sim.get_optimization(&id).await.unwrap();
    assert_eq!(d.summary.status, UiStatus::Running);
    assert!(d.summary.baseline.is_some() && d.noise_floor.unwrap() > 0.0);
    assert!(d.agents.iter().all(|a| a.status == AgentStatus::Running));
    assert!(d.calibration.is_some());

    for _ in 0..30 {
        sim.tick(&id).await.unwrap();
    }
    let d = sim.get_optimization(&id).await.unwrap();
    let c = sim.candidates(&id).await.unwrap();
    assert_eq!(c.len(), 60, "two agents, thirty turns");
    assert!(c.iter().all(|c| c.verdict != CandidateVerdict::Pending));
    // the invariant of the whole product: the best value only improves, and every move is a verified candidate
    let series = sim.ratchet_history(&id, None).await.unwrap();
    assert!(series.steps.windows(2).all(|w| w[1].value < w[0].value));
    let wins = c.iter().filter(|c| c.moved_ratchet).count() as i64;
    assert_eq!(d.summary.verified_wins, wins);
    assert!(wins >= 1, "thirty turns of two agents find something");
    for w in c.iter().filter(|c| c.moved_ratchet) {
        assert_eq!(w.verdict, CandidateVerdict::Verified);
        assert!(
            w.confirm_value.is_some(),
            "a win was confirmed on the hidden trace"
        );
    }
    // every verdict has a reason and spend accrues
    assert!(c.iter().all(|c| !c.reasons.is_empty()));
    assert!(d.summary.spend_usd > 10.0);

    // the events: a candidate event per turn, a ratchet event per win
    let mut cand = 0;
    let mut ratchet = 0;
    while let Ok(e) = events.try_recv() {
        match e {
            ApiEvent::Candidate {
                optimization_id, ..
            } if optimization_id == id => cand += 1,
            ApiEvent::Ratchet {
                optimization_id,
                from,
                to,
            } if optimization_id == id => {
                ratchet += 1;
                assert!(to < from, "minimize: the ratchet moved down");
            }
            _ => {}
        }
    }
    assert_eq!(cand, 60);
    assert_eq!(ratchet, wins);
}

#[tokio::test]
async fn paused_agents_skip_their_turn_and_messages_are_read_once() {
    let sim = SimService::manual().await.unwrap();
    let id = start_new(&sim, 100.0, 50).await;
    let agents = sim.get_optimization(&id).await.unwrap().agents;
    let (a, b) = (agents[0].id.clone(), agents[1].id.clone());
    sim.pause_agent(&a).await.unwrap();
    sim.message_agent(&b, "  look at the tokenizer  ")
        .await
        .unwrap();
    sim.tick(&id).await.unwrap();
    let c = sim.candidates(&id).await.unwrap();
    assert_eq!(c.len(), 1, "only the running agent took a turn");
    assert_eq!(c[0].agent_id, b);
    assert_eq!(c[0].message, "Following your note: look at the tokenizer");
    sim.tick(&id).await.unwrap();
    let c = sim.candidates(&id).await.unwrap();
    assert!(
        !c[1].message.starts_with("Following"),
        "the note is consumed"
    );
    sim.resume_agent(&a).await.unwrap();
    sim.tick(&id).await.unwrap();
    assert_eq!(sim.candidates(&id).await.unwrap().len(), 4);
    // the log shows the user's note and the status changes
    let log = sim.agent_events(&a, None, 50).await.unwrap();
    let texts: Vec<&str> = log.events.iter().map(|e| e.text.as_str()).collect();
    assert!(
        texts.contains(&"Paused by user") && texts.contains(&"Resumed"),
        "{texts:?}"
    );
    let log_b = sim.agent_events(&b, None, 50).await.unwrap();
    assert!(
        log_b
            .events
            .iter()
            .any(|e| e.text == "You: look at the tokenizer")
    );
    assert_eq!(
        sim.get_optimization(&id).await.unwrap().agents[0].status,
        AgentStatus::Running
    );
}

#[tokio::test]
async fn a_run_ends_when_the_budget_is_spent() {
    let sim = SimService::manual().await.unwrap();
    let id = start_new(&sim, 3.0, 500).await;
    let mut turns = 0;
    while sim.get_optimization(&id).await.unwrap().summary.status == UiStatus::Running {
        sim.tick(&id).await.unwrap();
        turns += 1;
        assert!(turns < 50, "the budget must stop the run");
    }
    let d = sim.get_optimization(&id).await.unwrap();
    assert_eq!(d.summary.status, UiStatus::Completed);
    assert!(d.summary.spend_usd >= 3.0);
    assert!(d.agents.iter().all(|a| a.status == AgentStatus::Done));
    let log = sim.agent_events(&d.agents[0].id, None, 5).await.unwrap();
    assert_eq!(log.events.last().unwrap().text, "Stopped: budget exhausted");
}

#[tokio::test]
async fn a_run_ends_on_a_plateau() {
    let sim = SimService::manual().await.unwrap();
    let id = start_new(&sim, 1000.0, 3).await;
    for _ in 0..200 {
        sim.tick(&id).await.unwrap();
        if sim.get_optimization(&id).await.unwrap().summary.status != UiStatus::Running {
            break;
        }
    }
    let d = sim.get_optimization(&id).await.unwrap();
    assert_eq!(d.summary.status, UiStatus::Completed);
    let log = sim.agent_events(&d.agents[0].id, None, 5).await.unwrap();
    assert!(
        log.events
            .last()
            .unwrap()
            .text
            .starts_with("Stopped: plateau")
    );
}

#[tokio::test]
async fn stop_ends_the_run_and_a_stopped_run_cannot_restart() {
    let sim = SimService::manual().await.unwrap();
    let id = start_new(&sim, 100.0, 50).await;
    sim.tick(&id).await.unwrap();
    sim.stop_optimization(&id, Some("enough".into()))
        .await
        .unwrap();
    let d = sim.get_optimization(&id).await.unwrap();
    assert_eq!(d.summary.status, UiStatus::Stopped);
    assert!(d.agents.iter().all(|a| a.status == AgentStatus::Stopped));
    assert!(d.ended_at.is_some());
    sim.tick(&id).await.unwrap(); // a late timer tick does nothing
    assert_eq!(sim.candidates(&id).await.unwrap().len(), 2);
    assert!(matches!(
        sim.start_optimization(&id).await,
        Err(ServiceError::Conflict(_))
    ));
    // starting twice while running is a no-op
    let other = start_new(&sim, 100.0, 50).await;
    sim.start_optimization(&other).await.unwrap();
    assert!(matches!(
        sim.start_optimization("nope").await,
        Err(ServiceError::NotFound(_))
    ));
}

#[tokio::test]
async fn pull_request_and_report_follow_the_wins() {
    let sim = SimService::manual().await.unwrap();
    let before = sim.get_optimization("opt_demo_ingest").await.unwrap();
    assert!(before.pr_url.is_none() && before.summary.status == UiStatus::Completed);
    let pr = sim.open_pull_request("opt_demo_ingest").await.unwrap();
    let after = sim.get_optimization("opt_demo_ingest").await.unwrap();
    assert_eq!(after.pr_url.as_deref(), Some(pr.url.as_str()));
    assert_eq!(
        after.summary.status,
        UiStatus::PrOpen,
        "completed with a pull request"
    );
    let r = sim.report("opt_demo_ingest").await.unwrap();
    assert_eq!(r.file_name, "ratchet-report-opt_demo_ingest.md");
    assert!(r.markdown.contains("**41.2 k/s \u{2192} 58.9 k/s** (43.0% better) with 2 verified wins out of 3 candidates."), "{}", r.markdown);
    assert!(r.markdown.contains("| 3 candidates") || r.markdown.contains("## All candidates"));
    assert!(r.markdown.contains("Total $61.20 of $100.00 budget"));
    assert!(matches!(
        sim.open_pull_request("opt_demo_web").await,
        Err(ServiceError::Conflict(_))
    ));
}

#[tokio::test]
async fn cost_reconcile_is_pending_while_running_and_idempotent_afterwards() {
    let sim = SimService::manual().await.unwrap();
    let live = sim.reconcile_costs("opt_demo_search").await.unwrap();
    assert_eq!(format!("{:?}", live.status), "Pending");
    assert!(
        live.reason
            .unwrap()
            .contains("reconcile after it has ended")
    );
    // the six-week-old run reconciles once, then there is nothing left to do
    let first = sim.reconcile_costs("opt_demo_edge").await.unwrap();
    assert_eq!(format!("{:?}", first.status), "Reconciled");
    assert!(first.written_usd > 0.0 && first.lines.iter().any(|l| l.written));
    let again = sim.reconcile_costs("opt_demo_edge").await.unwrap();
    assert_eq!(format!("{:?}", again.status), "Unchanged");
    assert_eq!(again.written_usd, 0.0);
    let spend = sim.spend("opt_demo_edge").await.unwrap();
    assert!(spend.actual > 0.0, "the corrections are billing truth");
    assert!(matches!(
        sim.reconcile_costs("nope").await,
        Err(ServiceError::NotFound(_))
    ));
}

#[tokio::test]
async fn the_environment_lives_with_its_runs() {
    let sim = SimService::manual().await.unwrap();
    let envs = sim.environments().await.unwrap();
    assert_eq!(envs.idle_minutes, 30);
    let e = &envs.environments[0];
    assert_eq!(e.phase, EnvironmentPhase::Ready);
    assert_eq!(e.active_runs, ["opt_demo_search"]);
    assert!(e.simulated && !e.can_end && e.destroy_at.is_none());
    assert!(e.spend_usd > 40.0);
    assert_eq!(
        envs.last_receipt.as_ref().unwrap().sandbox_id,
        "sbx-demo-0",
        "the seeded teardown"
    );
    let old = sim.get_optimization("opt_demo_ingest").await.unwrap();
    assert_eq!(
        old.receipt.unwrap().runs[0].optimization_id,
        "opt_demo_ingest"
    );

    let sbx = e.sandbox_id.clone();
    sim.stop_optimization("opt_demo_search", None)
        .await
        .unwrap();
    let idle = &sim.environments().await.unwrap().environments[0];
    assert_eq!(idle.phase, EnvironmentPhase::Idle);
    assert!(idle.can_end && idle.idle_since.is_some());
    assert_eq!(
        idle.destroy_at.unwrap() - idle.idle_since.unwrap(),
        30 * 60_000,
        "the idle window"
    );

    let after = sim.end_environment(&sbx).await.unwrap();
    assert!(after.environments.is_empty());
    let r = after.last_receipt.unwrap();
    assert_eq!(
        (r.sandbox_id.as_str(), r.trigger, r.nothing_running),
        (sbx.as_str(), ReceiptTrigger::Manual, true)
    );
    assert!(r.spend_usd > 40.0);
    // the run's detail shows the receipt now
    assert!(
        sim.get_optimization("opt_demo_search")
            .await
            .unwrap()
            .receipt
            .is_some()
    );
    assert!(matches!(
        sim.end_environment(&sbx).await,
        Err(ServiceError::Conflict(_))
    ));
    // a new run brings a new environment up
    start_new(&sim, 10.0, 5).await;
    assert_eq!(
        sim.environments().await.unwrap().environments[0].phase,
        EnvironmentPhase::Ready
    );
}

#[tokio::test]
async fn creating_validates_and_checks_readiness() {
    let sim = SimService::manual().await.unwrap();
    // a repo run needs GitHub
    let mut live = demo_input(10.0, 5);
    live.demo = None;
    live.repo_url = "https://github.com/acme/search-api.git".into();
    let id = sim.create_optimization(live.clone()).await.unwrap().id;
    let d = sim.get_optimization(&id).await.unwrap();
    assert_eq!(d.summary.repo, "acme/search-api");
    assert_eq!(d.summary.status, UiStatus::Draft);
    sim.github_sign_out().await.unwrap();
    match sim.create_optimization(live.clone()).await {
        Err(ServiceError::NotReady(m)) => assert!(m.contains("sign in with GitHub")),
        other => panic!("{other:?}"),
    }
    live.repo_url = "not a repo".into();
    assert!(matches!(
        sim.create_optimization(live).await,
        Err(ServiceError::InvalidFields { .. })
    ));
    // an unapproved environment draft blocks the run
    let draft = sim
        .create_environment_draft(CreateEnvironmentDraftInput {
            repo: "acme/search-api".into(),
            base_ref: "main".into(),
            goal: "Make search faster".into(),
            agent: None,
            scope: None,
        })
        .await
        .unwrap();
    let mut with_draft = demo_input(10.0, 5);
    with_draft.environment_draft_id = Some(draft.id.clone());
    assert!(matches!(
        sim.create_optimization(with_draft.clone()).await,
        Err(ServiceError::NotReady(_))
    ));
    sim.approve_environment_draft(&draft.id).await.unwrap();
    sim.create_optimization(with_draft).await.unwrap();
}

#[tokio::test]
async fn settings_connections_and_preflight_tell_the_truth() {
    let sim = SimService::manual().await.unwrap();
    let pre = |agents: Vec<ratchet_core::AgentKind>, w, src| PreflightInput {
        agents,
        agents_where: w,
        source: src,
        repo: None,
    };
    use ratchet_core::AgentKind::{ClaudeCode, Codex};
    use ratchet_core::AgentsWhere::{Cloud, Local};
    let items = sim
        .preflight(pre(vec![ClaudeCode], Local, PreflightSource::Github))
        .await
        .unwrap();
    assert!(
        items.iter().all(|i| i.state == PreflightState::Ok),
        "{items:?}"
    );
    let items = sim
        .preflight(pre(vec![Codex], Cloud, PreflightSource::Local))
        .await
        .unwrap();
    assert!(
        items
            .iter()
            .any(|i| i.id == "key-codex" && i.state == PreflightState::Fail)
    );
    let demo = sim
        .preflight(pre(vec![], Local, PreflightSource::Demo))
        .await
        .unwrap();
    assert_eq!(demo.len(), 1);
    // secrets never come back
    sim.set_secret(SecretName::OpenaiApiKey, "sk-proj-abcdefghijklmnop1234")
        .await
        .unwrap();
    let json = serde_json::to_string(&sim.settings().await.unwrap()).unwrap();
    assert!(!json.contains("sk-proj"));
    let items = sim
        .preflight(pre(vec![Codex], Cloud, PreflightSource::Local))
        .await
        .unwrap();
    assert!(
        items
            .iter()
            .any(|i| i.id == "key-codex" && i.state == PreflightState::Ok)
    );
    assert!(
        sim.test_connection(ConnectionTarget::Openai)
            .await
            .unwrap()
            .ok
    );
    sim.clear_secret(SecretName::OpenaiApiKey).await.unwrap();
    assert!(
        !sim.test_connection(ConnectionTarget::Openai)
            .await
            .unwrap()
            .ok
    );
    sim.set_aws_profile("").await.unwrap();
    assert!(!sim.test_connection(ConnectionTarget::Aws).await.unwrap().ok);
    assert!(matches!(
        sim.set_aws_profile("bad profile!").await,
        Err(ServiceError::InvalidFields { .. })
    ));
}

fn id_of(v: &ConnectorsVm, name: &str) -> String {
    v.connectors
        .iter()
        .find(|c| c.name() == name)
        .unwrap_or_else(|| panic!("no connector named {name}"))
        .id()
        .to_owned()
}

#[tokio::test]
async fn connectors_keep_their_invariants() {
    let sim = SimService::manual().await.unwrap();
    let all = sim.list_connectors().await.unwrap();
    assert!(all.encryption_available && all.client_id_configured && all.github_flow.is_none());
    // the accounts of design/v02/2-connectors.png: two GitHub, three AWS, the Claude Code login and a team key
    let kinds: Vec<_> = all.connectors.iter().map(ConnectorVm::kind).collect();
    assert_eq!(
        kinds,
        [
            ConnectorKind::Github,
            ConnectorKind::Github,
            ConnectorKind::Aws,
            ConnectorKind::Aws,
            ConnectorKind::Aws,
            ConnectorKind::Agent,
            ConnectorKind::Agent
        ]
    );
    for kind in [
        ConnectorKind::Github,
        ConnectorKind::Aws,
        ConnectorKind::Agent,
    ] {
        assert_eq!(
            all.connectors
                .iter()
                .filter(|c| c.kind() == kind && c.is_default())
                .count(),
            1,
            "one default per kind"
        );
    }
    // the staging SSO session has expired (the scripted probe), everything else works
    let staging = all
        .connectors
        .iter()
        .find(|c| c.name() == "acme-staging")
        .unwrap();
    assert_eq!(staging.status(), ConnectorStatus::Expired);
    assert_eq!(
        all.connectors
            .iter()
            .filter(|c| c.status() == ConnectorStatus::Expired)
            .count(),
        1
    );
    // no secret value anywhere
    let json = serde_json::to_string(&all).unwrap();
    assert!(
        !json.contains("SIMULATED") && !json.contains("sk-ant"),
        "{json}"
    );
}

#[tokio::test]
async fn connectors_can_be_added_renamed_defaulted_tested_and_removed() {
    let sim = SimService::manual().await.unwrap();
    let key = AddAgentConnectorInput::ApiKey {
        agent: ConnectorAgent::Codex,
        api_key: "sk-proj-abcdefghijklmnop1234".into(),
        name: Some("Codex \u{00b7} ci key".into()),
    };
    let v = sim.add_agent_connector(key).await.unwrap();
    let id = id_of(&v, "Codex \u{00b7} ci key");
    let c = v.connectors.iter().find(|c| c.id() == id).unwrap();
    match c {
        ConnectorVm::Agent {
            key_hint, runs_on, ..
        } => {
            assert_eq!(
                key_hint.as_deref(),
                Some("\u{00b7}\u{00b7}\u{00b7}\u{00b7}1234")
            );
            assert_eq!(runs_on.len(), 2, "a key also runs in the cloud");
        }
        other => panic!("{other:?}"),
    }
    assert!(
        !serde_json::to_string(&v)
            .unwrap()
            .contains("abcdefghijklmnop")
    );
    assert!(
        !c.is_default(),
        "the first key is not the default of a kind that already has one"
    );
    // making another the default moves the flag, removing the default promotes another
    let v = sim.set_default_connector(&id).await.unwrap();
    let agent_defaults = |v: &ConnectorsVm| {
        v.connectors
            .iter()
            .filter(|c| c.kind() == ConnectorKind::Agent && c.is_default())
            .map(|c| c.id().to_owned())
            .collect::<Vec<_>>()
    };
    assert_eq!(agent_defaults(&v), std::slice::from_ref(&id));
    let v = sim.remove_connector(&id).await.unwrap();
    assert_eq!(agent_defaults(&v).len(), 1, "another one was promoted");
    // rename, and the cap of an AWS profile
    let prod = id_of(&v, "acme-prod");
    let v = sim
        .update_connector(
            &prod,
            UpdateConnectorInput {
                name: Some("Acme production".into()),
                monthly_cap_usd: Some(None),
                ..Default::default()
            },
        )
        .await
        .unwrap();
    assert!(v.connectors.iter().any(|c| matches!(c,
        ConnectorVm::Aws { name, monthly_cap_usd: None, .. } if name == "Acme production")));
    // the staging session is expired; testing it again keeps it so, with the hint to sign in
    let staging = id_of(&v, "acme-staging");
    let t = sim.test_connector(&staging).await.unwrap();
    assert!(
        !t.ok && t.detail.contains("aws sso login --profile acme-staging"),
        "{}",
        t.detail
    );
    assert_eq!(t.connector.unwrap().status(), ConnectorStatus::Expired);
    // validation
    let e = sim
        .add_aws_connector(AddAwsConnectorInput {
            profile: "acme-prod".into(),
            region: None,
            monthly_cap_usd: None,
            name: None,
        })
        .await
        .unwrap_err();
    assert!(e.to_string().contains("already connected"), "{e}");
    let e = sim
        .add_aws_connector(AddAwsConnectorInput {
            profile: "nope".into(),
            region: None,
            monthly_cap_usd: None,
            name: None,
        })
        .await
        .unwrap_err();
    assert!(e.to_string().contains("not in ~/.aws"), "{e}");
    assert!(matches!(
        sim.remove_connector("ghost").await,
        Err(ServiceError::NotFound(_))
    ));
}

#[tokio::test]
async fn a_selection_can_only_name_connectors_that_can_do_the_job() {
    let sim = SimService::manual().await.unwrap();
    let v = sim.list_connectors().await.unwrap();
    let (gh, aws, login) = (
        id_of(&v, "srikanth235"),
        id_of(&v, "acme-prod"),
        id_of(&v, "Claude Code \u{00b7} your login"),
    );
    let sel = |github: &str, aws: &str, id: &str, where_| RepoConnections {
        github: github.into(),
        aws: aws.into(),
        agents: vec![AgentSelection {
            connector_id: id.into(),
            where_,
        }],
    };
    let cloud = ratchet_core::enums::AgentsWhere::Cloud;
    let local = ratchet_core::enums::AgentsWhere::Local;
    for (bad, what) in [
        (sel(&aws, &aws, &login, local), "GitHub account"),
        (sel(&gh, &gh, &login, local), "AWS"),
        (sel(&gh, &aws, &login, cloud), "cannot run in the cloud"),
    ] {
        let e = sim.save_repo_connections("acme/x", bad).await.unwrap_err();
        assert!(matches!(e, ServiceError::Invalid(_)), "{e:?}");
        assert!(e.to_string().contains(what), "{e}");
    }
    assert!(
        sim.repo_connections("acme/x")
            .await
            .unwrap()
            .saved
            .is_none()
    );
    let ok = sim
        .save_repo_connections("Acme/X", sel(&gh, &aws, &login, local))
        .await
        .unwrap();
    assert_eq!(ok.repo, "acme/x");
    assert!(ok.saved.is_some());
    // creating an optimization with connections remembers them and lets them decide the agents
    let mut input = demo_input(20.0, 25);
    input.demo = None;
    input.repo_url = "acme/y".into();
    input.agents = vec![ratchet_core::AgentKind::Codex];
    input.connections = Some(sel(&gh, &aws, &login, local));
    sim.create_optimization(input).await.unwrap();
    let remembered = sim.repo_connections("acme/y").await.unwrap();
    assert_eq!(remembered.saved.unwrap().github, gh);
}

#[tokio::test]
async fn the_simulated_device_flow_adds_an_account_when_approved() {
    let sim = SimService::manual().await.unwrap();
    let started = sim
        .start_github_connector_sign_in(StartGithubSignInInput::default())
        .await
        .unwrap();
    let flow = started.github_flow.unwrap();
    assert_eq!(
        (flow.state, flow.user_code.as_str()),
        (GithubFlowState::Pending, "WDJB-MJHT")
    );
    let cancelled = sim.cancel_github_connector_sign_in().await.unwrap();
    assert!(cancelled.github_flow.is_none());
    sim.start_github_connector_sign_in(StartGithubSignInInput::default())
        .await
        .unwrap();
    let mut events = sim.subscribe();
    sim.complete_github_flow();
    // the poller looks again within a second
    let mut done = false;
    for _ in 0..40 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if sim.list_connectors().await.unwrap().github_flow.is_none() {
            done = true;
            break;
        }
    }
    assert!(done, "the flow ended");
    let list = sim.list_connectors().await.unwrap();
    assert_eq!(
        list.connectors
            .iter()
            .filter(|c| c.kind() == ConnectorKind::Github)
            .count(),
        2,
        "the same account signing in again replaces its token"
    );
    let mut saw_github = false;
    while let Ok(e) = events.try_recv() {
        saw_github |= e == ApiEvent::Github { signed_in: true };
    }
    assert!(saw_github);
}

#[tokio::test]
async fn drafts_follow_their_life_cycle() {
    let sim = SimService::manual().await.unwrap();
    let mk = |goal: &str, scope| CreateEnvironmentDraftInput {
        repo: "acme-org/web".into(),
        base_ref: "main".into(),
        goal: goal.into(),
        agent: None,
        scope,
    };
    let single = sim
        .create_environment_draft(mk("Shrink the bundle", None))
        .await
        .unwrap();
    assert_eq!(single.scope, EnvScope::Single);
    assert_eq!(single.resources.len(), 1);
    let none = sim
        .create_environment_draft(mk("Tidy the docs", None))
        .await
        .unwrap();
    assert_eq!(
        (none.scope, none.resources.len(), none.cost.hourly_usd),
        (EnvScope::None, 0, 0.0)
    );
    assert!(
        matches!(
            sim.open_environment_draft_pr(&none.id).await,
            Err(ServiceError::Conflict(_))
        ),
        "no files, no PR"
    );
    let forced = sim
        .create_environment_draft(mk("Tidy the docs", Some(EnvScope::Full)))
        .await
        .unwrap();
    assert_eq!(forced.scope, EnvScope::Full);
    // revisions
    let rds = sim
        .revise_environment_draft(
            &forced.id,
            ReviseEnvironmentDraftInput {
                request: "use RDS instead of a Postgres container".into(),
                scope: None,
            },
        )
        .await
        .unwrap();
    assert_eq!(rds.revision, 2);
    assert!(rds.resources.iter().any(|r| r.name == "RDS Postgres 16"));
    assert!(rds.cost.hourly_usd > forced.cost.hourly_usd);
    let files = sim.environment_draft_files(&rds.id).await.unwrap();
    assert!(files.files[0].content.contains("aws_db_instance"));
    // validation
    for bad in [
        CreateEnvironmentDraftInput {
            goal: "  ".into(),
            ..mk("x", None)
        },
        CreateEnvironmentDraftInput {
            repo: "nope".into(),
            ..mk("x", None)
        },
    ] {
        assert!(matches!(
            sim.create_environment_draft(bad).await,
            Err(ServiceError::InvalidFields { .. })
        ));
    }
    // approving locks; approving again is a no-op
    let a = sim.approve_environment_draft(&rds.id).await.unwrap();
    assert!(a.approved_at.is_some());
    assert_eq!(
        sim.approve_environment_draft(&rds.id)
            .await
            .unwrap()
            .approved_at,
        a.approved_at
    );
    assert!(matches!(
        sim.revise_environment_draft(
            &rds.id,
            ReviseEnvironmentDraftInput {
                request: "x".into(),
                scope: None
            }
        )
        .await,
        Err(ServiceError::Conflict(_))
    ));
    assert!(matches!(
        sim.get_environment_draft("envd_nope").await,
        Err(ServiceError::NotFound(_))
    ));
}

#[tokio::test]
async fn the_ledger_file_survives_a_restart_and_runs_resume() {
    let dir = tempfile::tempdir().unwrap();
    let opts = SimOptions {
        ledger_path: Some(dir.path().join("sub/ratchet.db")),
        auto_run: false,
        ..SimOptions::default()
    };
    let id;
    {
        let sim = SimService::new(opts.clone()).await.unwrap();
        id = start_new(&sim, 50.0, 50).await;
        sim.tick(&id).await.unwrap();
        assert_eq!(
            sim.ledger_path().unwrap(),
            dir.path().join("sub/ratchet.db").display().to_string()
        );
        sim.shutdown();
    }
    // the seed is not applied twice, the created run is still there and resumes where it was
    let sim = SimService::new(opts).await.unwrap();
    let snap = sim.list_optimizations(KpiRange::Days30).await.unwrap();
    assert_eq!(snap.optimizations.len(), 5);
    let c = sim.candidates(&id).await.unwrap();
    assert_eq!(c.len(), 2);
    sim.tick(&id).await.unwrap();
    assert_eq!(
        sim.candidates(&id).await.unwrap().len(),
        4,
        "the run was resumed"
    );
    let row = sim
        .ledger()
        .get_optimization(&OptimizationId::new(&id))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(row.status, OptimizationStatus::Running);
}

#[tokio::test]
async fn timers_drive_a_run_without_ticks() {
    let sim = SimService::new(SimOptions {
        auto_run: true,
        tick_ms: 20,
        baseline_ms: 20,
        github_flow_ms: 20,
        resume_runs: false,
        ..SimOptions::default()
    })
    .await
    .unwrap();
    let id = sim
        .create_optimization(demo_input(100.0, 50))
        .await
        .unwrap()
        .id;
    sim.start_optimization(&id).await.unwrap();
    // still baselining right after the call
    let early = sim.get_optimization(&id).await.unwrap();
    assert!(matches!(
        early.summary.status,
        UiStatus::Baselining | UiStatus::Running
    ));
    let mut waited = 0;
    loop {
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
        waited += 1;
        if sim.candidates(&id).await.unwrap().len() >= 4 {
            break;
        }
        assert!(waited < 200, "the timers must produce candidates");
    }
    sim.stop_optimization(&id, None).await.unwrap();
    let n = sim.candidates(&id).await.unwrap().len();
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(
        sim.candidates(&id).await.unwrap().len(),
        n,
        "nothing runs after stop"
    );
    // the GitHub device flow completes by itself
    sim.github_sign_out().await.unwrap();
    let st = sim.github_sign_in().await.unwrap();
    assert_eq!(st.flow.unwrap().state, GithubFlowState::Pending);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    let st = sim.github_status().await.unwrap();
    assert!(st.signed_in && st.flow.is_none());
    sim.shutdown();
}

#[tokio::test]
async fn repos_and_inspection() {
    let sim = SimService::manual().await.unwrap();
    let dir = tempfile::tempdir().unwrap();
    // a local folder with a ratchet.yaml and a .git directory
    std::fs::create_dir(dir.path().join(".git")).unwrap();
    std::fs::write(
        dir.path().join("ratchet.yaml"),
        ratchetd::sim::DEFAULT_CONFIG_YAML,
    )
    .unwrap();
    let r = sim
        .inspect_repo(
            RepoSource::Local {
                path: dir.path().display().to_string(),
            },
            None,
        )
        .await
        .unwrap();
    assert!(r.problem.is_none() && r.config.is_some() && r.templates.is_empty());
    // not a git repository
    let plain = tempfile::tempdir().unwrap();
    let r = sim
        .inspect_repo(
            RepoSource::Local {
                path: plain.path().display().to_string(),
            },
            None,
        )
        .await
        .unwrap();
    assert!(r.problem.unwrap().contains("not a git repository"));
    assert!(
        r.templates.is_empty(),
        "nothing is offered for a folder that cannot be used"
    );
    let r = sim
        .inspect_repo(
            RepoSource::Local {
                path: "/definitely/not/here".into(),
            },
            None,
        )
        .await
        .unwrap();
    assert!(r.problem.unwrap().contains("not a folder"));
    // a broken ratchet.yaml surfaces its errors
    std::fs::write(plain.path().join("ratchet.yaml"), "version: 0\n").unwrap();
    std::fs::create_dir(plain.path().join(".git")).unwrap();
    let r = sim
        .inspect_repo(
            RepoSource::Local {
                path: plain.path().display().to_string(),
            },
            None,
        )
        .await
        .unwrap();
    assert!(r.config.is_none() && !r.errors.is_empty());
    assert!(matches!(
        sim.inspect_repo(
            RepoSource::Github {
                repo: "acme/unknown".into()
            },
            None,
        )
        .await,
        Err(ServiceError::NotFound(_))
    ));
    assert!(matches!(
        sim.inspect_repo(
            RepoSource::Github {
                repo: "no slash".into()
            },
            None,
        )
        .await,
        Err(ServiceError::InvalidFields { .. })
    ));
}

#[tokio::test]
async fn template_proposals_use_the_core_templates() {
    let sim = SimService::manual().await.unwrap();
    let input = |id: &str| ratchet_core::ports::ProposeTemplateInput {
        repo_url: "https://github.com/acme-org/shop-api".into(),
        base_ref: "main".into(),
        template_id: id.into(),
        metric: None,
        direction: None,
        value_usd: None,
        overwrite: None,
    };
    let p = sim.propose_template(input("perf")).await.unwrap();
    assert_eq!(
        p.template_id, "cli-cold-start",
        "the legacy pack name resolves"
    );
    assert!(p.files.contains(&"RATCHET.md".to_owned()) && p.files[0] == "ratchet.yaml");
    assert!(
        p.pr_url
            .starts_with("https://github.com/acme-org/shop-api/pull/")
    );
    assert_eq!(p.sha.len(), 40);
    assert!(matches!(
        sim.propose_template(input("demo-search")).await,
        Err(ServiceError::Invalid(_))
    ));
    assert!(matches!(
        sim.propose_template(input("nope")).await,
        Err(ServiceError::NotFound(_))
    ));
}
