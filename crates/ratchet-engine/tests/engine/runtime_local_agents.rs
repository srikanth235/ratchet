//! `packages/runtime/src/local-agents.int.test.ts`: the "This machine" wiring end to end, without AWS: the real engine, the real
//! local agent control with the real Claude Code adapter (driving a stand-in `claude` script), a git worktree per agent against a
//! local "GitHub", and the automatic sandbox (mock provider) created at the first run and removed after the idle window with a
//! receipt.

use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ratchet_agents::{
    HarnessAuth, HarnessInfo, HarnessKind, LocalAdapterOptions, LocalAgentControlOptions,
    LoopTuning, create_local_agent_control,
};
use ratchet_core::config::RatchetConfig;
use ratchet_core::enums::{
    AgentKind, AgentsWhere, CandidateVerdict, Mode, OptimizationStatus, SandboxStatus,
};
use ratchet_core::ids::OptimizationId;
use ratchet_core::ports::{
    BaselineMetrics, BenchJob, BenchJobPhase, BenchOutcome, BenchRun, BenchRunner,
    CancellationToken, CloudCompute, EngineEvent, PolledEvents, PortError, PortResult,
    RunnerConfig, RunnerControl, RunnerHandle, RunnerMessage, SandboxPhase, SandboxProvider,
    TeardownTrigger,
};
use ratchet_engine::runtime::system_clock;
use ratchet_engine::{
    CreateOptimizationInput, Engine, EngineOptions, EnginePorts, EngineSettings, GitSource,
    SandboxSettings,
};
use ratchet_github::{
    CreateBareRemoteOptions, LocalGitHost, LocalGitHostOptions, create_bare_remote,
};
use ratchet_ledger::{Ledger, LedgerOptions};
use ratchet_sandbox::{
    FsArtifactStore, MockCloudCompute, MockCloudComputeOptions, MockSandboxProvider,
    StaticPriceBook,
};

const CONFIG: &str = "version: 0
objective:
  metric: latency_ms
  direction: minimize
  min_gain: 0.03
bench:
  ref: ratchet/bench
  run: bench.sh
  repeats: 3
  hardware: c7i.2xlarge
guardrails:
  - run: \"true\"
protected:
  - bench/**
  - ratchet.yaml
agent:
  instructions: RATCHET.md
  allowed_instance_types: [c7i.xlarge]
";

const STREAM: [&str; 3] = [
    r#"{"type":"system","subtype":"init","session_id":"sess-1","model":"shim"}"#,
    r#"{"type":"assistant","message":{"content":[{"type":"text","text":"made it faster"}]}}"#,
    r#"{"type":"result","subtype":"success","is_error":false,"result":"done","num_turns":1,"duration_ms":5,"total_cost_usd":0.03,"usage":{"input_tokens":10,"output_tokens":5}}"#,
];

async fn until<F, Fut>(mut cond: F, ms: u64, what: &str)
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = bool>,
{
    let t0 = Instant::now();
    while !cond().await {
        assert!(
            t0.elapsed() < Duration::from_millis(ms),
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

/// 100 for the baseline, 80 for everything else.
struct FixedBench;

#[async_trait]
impl BenchRunner for FixedBench {
    async fn run(
        &self,
        job: &BenchJob,
        _c: &RatchetConfig,
        _b: Option<&BaselineMetrics>,
        _x: Option<&CancellationToken>,
    ) -> PortResult<BenchOutcome> {
        let v = if job.phase == BenchJobPhase::Baseline {
            100.0
        } else {
            80.0
        };
        let at = system_clock().now();
        Ok(BenchOutcome {
            runs: (0..3)
                .map(|_| BenchRun {
                    value: v,
                    metrics: [("objective".to_owned(), v)].into(),
                    duration_ms: 10,
                })
                .collect(),
            samples: vec![v; 3],
            guardrails: vec![],
            logs_key: None,
            started_at: at,
            finished_at: at + 10,
        })
    }
}

struct NoCloudRunners;

#[async_trait]
impl RunnerControl for NoCloudRunners {
    async fn start(&self, _h: &RunnerHandle, _c: &RunnerConfig) -> PortResult<()> {
        Err(PortError::failed("cloud runners must not be used"))
    }
    async fn send(&self, _h: &RunnerHandle, _m: &RunnerMessage) -> PortResult<()> {
        Ok(())
    }
    async fn poll(&self, _h: &RunnerHandle, _c: Option<&str>) -> PortResult<PolledEvents> {
        Ok(PolledEvents {
            events: vec![],
            cursor: "0".into(),
        })
    }
    async fn stop(&self, _h: &RunnerHandle) -> PortResult<()> {
        Ok(())
    }
}

#[tokio::test]
async fn runs_a_claude_code_agent_in_a_worktree_verifies_its_commit_then_removes_the_worktree_and_after_the_idle_window_the_sandbox()
 {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let project = root.join("project");
    std::fs::create_dir_all(project.join("src")).unwrap();
    std::fs::write(project.join("ratchet.yaml"), CONFIG).unwrap();
    std::fs::write(project.join("RATCHET.md"), "# Brief\nMake it faster.\n").unwrap();
    std::fs::write(project.join("src/app.txt"), "slow\n").unwrap();
    let remote = create_bare_remote(
        &project,
        CreateBareRemoteOptions {
            remote_dir: Some(root.join("remote.git")),
            work_dir: Some(root.join("seed")),
            ..Default::default()
        },
    )
    .await
    .unwrap();
    let git = Arc::new(
        LocalGitHost::new(LocalGitHostOptions::new(&remote.remote_dir).pr_dir(root.join("prs")))
            .unwrap(),
    );

    // a stand-in for the user's `claude`: signed in, edits the worktree once, logs the key it was (not) given
    let bin = root.join("bin");
    std::fs::create_dir(&bin).unwrap();
    let seen = root.join("seen.txt");
    let mut script = vec![
        "#!/bin/sh".to_owned(),
        r#"case "$1" in --version) echo "2.1.4 (Claude Code)"; exit 0;; esac"#.to_owned(),
        "cat >/dev/null".to_owned(),
        format!(
            r#"printf 'cwd=%s key=%s\n' "$PWD" "$ANTHROPIC_API_KEY" >> '{}'"#,
            seen.display()
        ),
        r#"case "$*" in *--resume*) ;; *) echo fast > src/app.txt;; esac"#.to_owned(),
    ];
    script.extend(STREAM.iter().map(|l| format!("echo '{l}'")));
    script.push(String::new());
    std::fs::write(bin.join("claude"), script.join("\n")).unwrap();
    std::fs::set_permissions(bin.join("claude"), std::fs::Permissions::from_mode(0o755)).unwrap();
    let harness = HarnessInfo {
        kind: HarnessKind::ClaudeCode,
        installed: true,
        version: Some("2.1.4".into()),
        path: Some(bin.join("claude").display().to_string()),
        auth: HarnessAuth::SignedIn,
        hint: String::new(),
    };
    let agents_root = root.join("agents");
    let local_runners = Arc::new(create_local_agent_control(LocalAgentControlOptions {
        adapters: LocalAdapterOptions {
            harnesses: vec![harness],
            login_path: "/usr/bin:/bin".into(),
            base_env: Some(HashMap::from([
                ("HOME".to_owned(), root.display().to_string()),
                (
                    "ANTHROPIC_API_KEY".to_owned(),
                    "sk-ant-MUSTNOTPASS".to_owned(),
                ),
            ])),
            ..Default::default()
        },
        root_dir: agents_root.clone(),
        loop_tuning: LoopTuning {
            max_idle_turns: Some(1),
            retry_delay: Some(Duration::from_millis(1)),
            ..Default::default()
        },
        ..Default::default()
    }));

    let ledger = Ledger::open_memory(LedgerOptions::default()).await.unwrap();
    let compute = Arc::new(MockCloudCompute::new(MockCloudComputeOptions::with_clock(
        system_clock(),
    )));
    let sandbox_provider = Arc::new(MockSandboxProvider::default());
    let engine = Engine::new(EngineOptions {
        ledger: ledger.clone(),
        mode: Mode::Live,
        clock: system_clock(),
        ports: EnginePorts {
            sandbox: sandbox_provider.clone(),
            compute: compute.clone(),
            prices: Arc::new(StaticPriceBook::new([])),
            artifacts: Arc::new(FsArtifactStore::new(root.join("artifacts"))),
            git: GitSource::One(git),
            runners: Arc::new(NoCloudRunners),
            local_runners: Some(local_runners.clone()),
            bench: Arc::new(FixedBench),
            costs: None,
            environment: None,
        },
        settings: EngineSettings {
            tick_interval_ms: Some(40),
            scratch_dir: Some(root.join("scratch")),
            agents_where: Some(AgentsWhere::Local),
            sandbox: SandboxSettings {
                auto: Some(true),
                idle_ms: Some(600),
                retry_ms: Some(100),
                ..Default::default()
            },
            noise: ratchet_core::NoiseFloorOptions {
                k: Some(0.25),
                ..Default::default()
            },
            ..Default::default()
        },
    })
    .await;
    let mut rx = engine.subscribe();
    let stop = CancellationToken::new();
    let looped = {
        let (e, s) = (engine.clone(), stop.clone());
        tokio::spawn(async move { e.run(Some(s)).await })
    };

    let id = engine
        .create_optimization(CreateOptimizationInput {
            id: Some("run-1".into()),
            repo_url: remote.remote_dir.display().to_string(),
            base_ref: "main".into(),
            agents: vec![AgentKind::ClaudeCode],
            budget_usd: 20.0,
            time_limit_seconds: 600,
            plateau_evals: 5,
            ..Default::default()
        })
        .await
        .unwrap();
    let oid = OptimizationId::new(&id);
    assert!(
        ledger
            .get_optimization(&oid)
            .await
            .unwrap()
            .unwrap()
            .sandbox_id
            .is_none()
    );
    engine.start(&id).await.unwrap();

    // the sandbox appeared with the run, and the agent runs on this machine
    assert_eq!(
        ledger
            .get_optimization(&oid)
            .await
            .unwrap()
            .unwrap()
            .sandbox_id
            .as_ref()
            .map(|s| s.as_str().to_owned())
            .as_deref(),
        Some("ratchet-sandbox")
    );
    assert_eq!(
        ledger
            .get_sandbox("ratchet-sandbox")
            .await
            .unwrap()
            .unwrap()
            .status,
        SandboxStatus::Applied
    );
    assert!(engine.run_active());
    let agent = ledger.list_agents(&oid).await.unwrap().remove(0);
    assert_eq!(
        agent.runner_instance_id.as_deref(),
        Some(format!("local:{}", agent.id).as_str())
    );
    assert_eq!(compute_instances(&compute).await, 0);

    until(
        || async {
            ledger.get_optimization(&oid).await.unwrap().unwrap().status
                == OptimizationStatus::Stopped
        },
        60_000,
        "the run to finish by itself (all agents done)",
    )
    .await;
    let candidates = ledger.candidates_with_results(&oid).await.unwrap();
    assert_eq!(
        candidates
            .iter()
            .map(|c| c.candidate.verdict)
            .collect::<Vec<_>>(),
        [CandidateVerdict::Verified]
    );
    assert_eq!(ledger.current_best(&oid).await.unwrap(), Some(80.0));
    let seen_text = std::fs::read_to_string(&seen).unwrap();
    assert!(
        seen_text.contains(&format!(
            "cwd={}",
            agents_root.join(&id).join(agent.id.as_str()).display()
        )),
        "{seen_text}"
    );
    assert!(seen_text.contains(" key=\n"), "{seen_text}");
    assert!(!seen_text.contains("MUSTNOTPASS"));
    // stopping the agent removed its worktree; the clone stays until the run is torn down
    let wt = agents_root.join(&id).join(agent.id.as_str());
    until(
        || async { !wt.exists() },
        5000,
        "the worktree to be removed",
    )
    .await;
    assert!(agents_root.join(&id).join("clone").join(".git").exists());
    assert!(!engine.run_active());

    // idle window -> torn down by the tick loop, with a receipt
    until(
        || async {
            ledger
                .get_sandbox("ratchet-sandbox")
                .await
                .unwrap()
                .unwrap()
                .status
                == SandboxStatus::Destroyed
        },
        20_000,
        "the idle teardown",
    )
    .await;
    let receipt = engine
        .teardown_receipt(&id)
        .await
        .unwrap()
        .expect("a receipt");
    assert_eq!(receipt.sandbox_id, "ratchet-sandbox");
    assert_eq!(receipt.trigger, TeardownTrigger::Idle);
    assert!(receipt.nothing_running);
    assert_eq!(receipt.runs.len(), 1);
    assert_eq!(
        (
            receipt.runs[0].optimization_id.as_str(),
            receipt.runs[0].status.as_str()
        ),
        (id.as_str(), "completed")
    );
    assert!(
        (receipt.spend_usd - 0.06).abs() < 1e-5,
        "{}",
        receipt.spend_usd
    ); // two agent turns at the CLI's own cost report ($0.03 each)
    assert!(!agents_root.join(&id).exists()); // teardown released the local clone
    assert!(
        sandbox_provider
            .outputs("ratchet-sandbox")
            .await
            .unwrap()
            .is_none()
    );

    let mut events = Vec::new();
    while let Ok(e) = rx.try_recv() {
        events.push(e);
    }
    let active: Vec<bool> = events
        .iter()
        .filter_map(|e| {
            if let EngineEvent::RunActive { active, .. } = e {
                Some(*active)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(active, [true, false]);
    let phases: Vec<SandboxPhase> = events
        .iter()
        .filter_map(|e| {
            if let EngineEvent::Sandbox { phase, .. } = e {
                Some(*phase)
            } else {
                None
            }
        })
        .collect();
    assert_eq!(
        phases,
        [
            SandboxPhase::Creating,
            SandboxPhase::Ready,
            SandboxPhase::Idle,
            SandboxPhase::Destroying,
            SandboxPhase::Destroyed
        ]
    );

    stop.cancel();
    let _ = looped.await;
    engine.close().await;
    local_runners.stop_all().await;
}

async fn compute_instances(c: &MockCloudCompute) -> usize {
    c.list_tagged(None).await.unwrap().len()
}
