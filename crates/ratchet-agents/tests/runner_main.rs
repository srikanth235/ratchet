mod common;

use ratchet_core::ports::{AgentAdapter, ArtifactStore, RunnerControl, RunnerHandle};
// the same directory the binary uses for RATCHET_ARTIFACT_DIR (this is the one test binary that links the AWS SDK)
use ratchet_sandbox::FsArtifactStore;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use common::{
    AutoOptions, FakeCompute, Sandbox, auto_verdicts, base_config, base_config_with, env_of,
    make_sandbox_branches,
};

use ratchet_agents::git::{GitCall, GitResult, GitRunner};
use ratchet_agents::testing::{FakeProcessSpec, FakeSpawner};
use ratchet_agents::{
    AgentScript, ExecGit, RunnerMainOptions, ScriptStep, ScriptedAgent, SsmRunnerControl,
    SsmRunnerControlOptions, done_key, git_auth_env, parse_secret_refs, runner_main, runner_prefix,
    validate_config,
};
use serde_json::json;
use tokio::sync::mpsc;

fn handle() -> RunnerHandle {
    RunnerHandle {
        optimization_id: "opt1".into(),
        agent_id: "cc".into(),
        instance_id: "i-1".into(),
    }
}

fn script() -> AgentScript {
    AgentScript::new(
        "main",
        vec![
            vec![
                ScriptStep::Write {
                    path: "src/app.txt".into(),
                    content: "faster\n".into(),
                },
                ScriptStep::Usage {
                    input_tokens: 30_000,
                    output_tokens: 1_500,
                    cost_usd: None,
                },
            ],
            vec![ScriptStep::Write {
                path: "src/more.txt".into(),
                content: "more\n".into(),
            }],
        ],
    )
}

struct Setup {
    sb: Sandbox,
    _work: tempfile::TempDir,
    store: Arc<FsArtifactStore>,
    config: ratchet_core::ports::RunnerConfig,
    env: HashMap<String, String>,
    work: PathBuf,
}

fn setup() -> Setup {
    let sb = make_sandbox_branches(&["ratchet/cc"]);
    let tmp = tempfile::Builder::new()
        .prefix("runner-main-")
        .tempdir()
        .unwrap();
    let work = tmp.path().canonicalize().unwrap();
    let artifacts = work.join("artifacts");
    let script_file = work.join("script.json");
    std::fs::write(&script_file, serde_json::to_string(&script()).unwrap()).unwrap();
    let config = base_config(&sb, |c| {
        c["agentId"] = "cc".into();
        c["branch"] = "ratchet/cc".into();
    });
    let env = env_of(&[
        ("RATCHET_OPTIMIZATION_ID", "opt1"),
        ("RATCHET_AGENT_ID", "cc"),
        ("RATCHET_ARTIFACT_DIR", &artifacts.to_string_lossy()),
        ("RATCHET_WORKDIR", &work.join("wd/cc").to_string_lossy()),
        ("RATCHET_MOCK_SCRIPT", &script_file.to_string_lossy()),
        ("RATCHET_EVENT_FLUSH_MS", "20"),
        ("RATCHET_INBOX_POLL_MS", "10"),
        ("RATCHET_MAX_IDLE_TURNS", "1"),
        ("PATH", &common::path_env()),
    ]);
    Setup {
        sb,
        store: Arc::new(FsArtifactStore::new(&artifacts)),
        _work: tmp,
        config,
        env,
        work,
    }
}

fn ssm_over(store: Arc<FsArtifactStore>) -> SsmRunnerControl {
    SsmRunnerControl::new(SsmRunnerControlOptions {
        compute: Arc::new(FakeCompute::default()),
        store,
        runner_command: None,
        runner_env: Default::default(),
        state_dir: None,
    })
}

/// The engine side of the live path: `SsmRunnerControl` over the same store, answering commits with verdicts.
async fn engine_loop(control: &SsmRunnerControl, finished: &AtomicBool) {
    let (tx, mut rx) = mpsc::unbounded_channel();
    let answer = auto_verdicts(move |m| drop(tx.send(m)), AutoOptions::default());
    let mut cursor: Option<String> = None;
    while !finished.load(Ordering::SeqCst) {
        let r = control.poll(&handle(), cursor.as_deref()).await.unwrap();
        cursor = Some(r.cursor);
        r.events.iter().for_each(|e| answer(e));
        while let Ok(m) = rx.try_recv() {
            control.send(&handle(), &m).await.unwrap();
        }
        tokio::time::sleep(Duration::from_millis(15)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runner_main_reads_config_from_the_store_runs_the_loop_streams_events_answers_inbox_messages_writes_done_json()
 {
    let s = setup();
    let control = ssm_over(s.store.clone());
    control.start(&handle(), &s.config).await.unwrap(); // writes config.json (launch itself is faked)
    let finished = Arc::new(AtomicBool::new(false));
    let logs = Arc::new(Mutex::new(Vec::<String>::new()));
    let run = {
        let (finished, logs, env) = (finished.clone(), logs.clone(), s.env.clone());
        let store: Arc<dyn ArtifactStore> = s.store.clone();
        tokio::spawn(async move {
            let code = runner_main(RunnerMainOptions {
                env,
                store: Some(store),
                log: Some(Arc::new(move |l: &str| {
                    logs.lock().unwrap().push(l.to_string())
                })),
                ..RunnerMainOptions::default()
            })
            .await
            .unwrap();
            finished.store(true, Ordering::SeqCst);
            code
        })
    };
    engine_loop(&control, &finished).await;
    assert_eq!(run.await.unwrap(), 0);

    let events = control.poll(&handle(), None).await.unwrap().events;
    use ratchet_core::ports::AgentEventType::*;
    assert_eq!(events.iter().filter(|e| e.event_type == Commit).count(), 2);
    assert_eq!(events.iter().filter(|e| e.event_type == Verdict).count(), 2);
    let last = events.last().unwrap();
    assert_eq!(
        (last.event_type, last.text.as_deref()),
        (Status, Some("runner finished: idle"))
    );
    let done: serde_json::Value = serde_json::from_str(
        &s.store
            .get(&done_key(&runner_prefix("opt1", "cc")))
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(
        (&done["reason"], &done["candidates"]),
        (&json!("idle"), &json!(2))
    );
    assert!(done["finishedAt"].is_number());
    assert!(
        logs.lock()
            .unwrap()
            .last()
            .unwrap()
            .contains("finished: idle (2 candidates"),
        "{:?}",
        logs.lock().unwrap()
    );
    // candidates really landed on the agent branch
    common::sh(&s.sb.engine, &["fetch", "--quiet", "origin"]);
    let log = common::sh(
        &s.sb.engine,
        &["log", "--format=%B", "main..origin/ratchet/cc"],
    );
    assert_eq!(log.matches("Ratchet-Candidate: true").count(), 2);
}

/// Pretends github.com is the local sandbox remote, but records the environment git ran with.
struct UrlMappedGit {
    from: String,
    to: String,
    envs: Mutex<Vec<Option<HashMap<String, String>>>>,
}

#[async_trait]
impl GitRunner for UrlMappedGit {
    async fn run(&self, args: &[String], call: &GitCall) -> GitResult {
        self.envs.lock().unwrap().push(call.env.clone());
        let mapped: Vec<String> = args
            .iter()
            .map(|a| {
                if *a == self.from {
                    self.to.clone()
                } else {
                    a.clone()
                }
            })
            .collect();
        ExecGit.run(&mapped, call).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn runner_main_fetches_ssm_secrets_for_the_agent_never_the_git_token_and_authenticates_git_with_the_token()
 {
    let s = setup();
    let https_url = "https://github.com/acme/repo.git";
    let mut config = serde_json::to_value(&s.config).unwrap();
    config["repoUrl"] = https_url.into();
    s.store
        .put(
            &format!("{}config.json", runner_prefix("opt1", "cc")),
            config.to_string().as_bytes(),
        )
        .await
        .unwrap();
    let aws = FakeSpawner::with_plan(|call, _| {
        FakeProcessSpec::stdout(
            if call
                .args
                .iter()
                .any(|a| a == "/ratchet/opt1/cc/ANTHROPIC_API_KEY")
            {
                "sk-ant-xyz\n"
            } else {
                "ghs_token123\n"
            },
        )
    });
    let git = Arc::new(UrlMappedGit {
        from: https_url.into(),
        to: s.config.repo_url.clone(),
        envs: Mutex::new(Vec::new()),
    });
    let agent_env: Arc<Mutex<HashMap<String, String>>> = Arc::default();
    let captured = agent_env.clone();
    let mut env = s.env.clone();
    env.extend(env_of(&[
        ("RATCHET_SECRET_REFS", "ANTHROPIC_API_KEY=/ratchet/opt1/cc/ANTHROPIC_API_KEY,GITHUB_TOKEN=/ratchet/opt1/cc/GITHUB_TOKEN"),
        ("AWS_REGION", "us-east-1"),
        ("SECRET_OTHER", "leak-me-not"),
        ("HTTPS_PROXY", "http://10.42.3.10:3128"),
        ("NO_PROXY", "169.254.169.254"),
        ("NODE_USE_ENV_PROXY", "1"),
    ]));
    let code = runner_main(RunnerMainOptions {
        env,
        store: Some(s.store.clone()),
        spawner: Some(Arc::new(aws.clone())),
        git: Some(git.clone()),
        adapter_factory: Some(Arc::new(move |_, e| {
            *captured.lock().unwrap() = e.clone();
            Arc::new(ScriptedAgent::new(AgentScript {
                name: "x".into(),
                attempts: vec![],
            })) as Arc<dyn AgentAdapter>
        })),
        log: Some(Arc::new(|_| {})),
        ..RunnerMainOptions::default()
    })
    .await
    .unwrap();
    assert_eq!(code, 1); // empty script -> agent-unavailable
    let calls = aws.calls();
    assert_eq!(
        calls
            .iter()
            .map(|c| c.args[..2].join(" "))
            .collect::<Vec<_>>(),
        ["ssm get-parameter", "ssm get-parameter"]
    );
    assert!(calls[0].args.iter().any(|a| a == "--with-decryption"));
    let agent_env = agent_env.lock().unwrap();
    assert_eq!(
        agent_env.get("ANTHROPIC_API_KEY").map(String::as_str),
        Some("sk-ant-xyz")
    );
    assert!(!agent_env.contains_key("GITHUB_TOKEN"));
    assert!(!agent_env.contains_key("SECRET_OTHER")); // only a small passthrough list reaches the agent
    assert_eq!(
        agent_env.get("AWS_REGION").map(String::as_str),
        Some("us-east-1")
    );
    // With the sandbox egress proxy on, the agent CLIs reach the model APIs only through it.
    assert_eq!(
        agent_env.get("HTTPS_PROXY").map(String::as_str),
        Some("http://10.42.3.10:3128")
    );
    assert_eq!(
        agent_env.get("NO_PROXY").map(String::as_str),
        Some("169.254.169.254")
    );
    assert_eq!(
        agent_env.get("NODE_USE_ENV_PROXY").map(String::as_str),
        Some("1")
    );
    let envs = git.envs.lock().unwrap();
    let header = envs
        .iter()
        .flatten()
        .find(|e| e.contains_key("GIT_CONFIG_KEY_0"))
        .expect("git ran with the auth header");
    assert_eq!(
        header["GIT_CONFIG_KEY_0"],
        "http.https://github.com/.extraheader"
    );
    use base64::Engine as _;
    assert_eq!(
        header["GIT_CONFIG_VALUE_0"],
        format!(
            "AUTHORIZATION: basic {}",
            base64::engine::general_purpose::STANDARD.encode("x-access-token:ghs_token123")
        )
    );
}

#[tokio::test]
async fn runner_main_validates_its_inputs_with_actionable_errors() {
    let s = setup();
    let err_with = |env: HashMap<String, String>, with_store: bool| {
        let store: Option<Arc<dyn ArtifactStore>> =
            with_store.then(|| s.store.clone() as Arc<dyn ArtifactStore>);
        async move {
            runner_main(RunnerMainOptions {
                env,
                store,
                log: Some(Arc::new(|_| {})),
                ..RunnerMainOptions::default()
            })
            .await
            .unwrap_err()
            .to_string()
        }
    };
    let err = |env: HashMap<String, String>| err_with(env, true);
    let without = |name: &str| {
        let mut env = s.env.clone();
        env.remove(name);
        env
    };
    assert_eq!(
        err(without("RATCHET_OPTIMIZATION_ID")).await,
        "RATCHET_OPTIMIZATION_ID is required"
    );
    assert!(
        err_with(without("RATCHET_ARTIFACT_DIR"), false)
            .await
            .contains("RATCHET_ARTIFACTS_BUCKET")
    );
    assert!(err(s.env.clone()).await.contains("no runner config"));
    let with_config = |value: String| {
        let mut env = s.env.clone();
        env.insert("RATCHET_RUNNER_CONFIG".into(), value);
        env
    };
    assert!(
        err(with_config(r#"{"agentId":"cc"}"#.into()))
            .await
            .contains("missing: optimizationId")
    );
    let other = base_config_with("x", |c| c["agentId"] = "other".into());
    assert!(
        err(with_config(serde_json::to_string(&other).unwrap()))
            .await
            .contains("does not match")
    );
    assert!(
        parse_secret_refs(Some("NOEQUALS"))
            .unwrap_err()
            .to_string()
            .contains("bad RATCHET_SECRET_REFS")
    );
    assert_eq!(
        parse_secret_refs(Some(" A=/x/a , B=/y/b=c ")).unwrap(),
        vec![
            ("A".to_string(), "/x/a".to_string()),
            ("B".to_string(), "/y/b=c".to_string())
        ]
    );
    assert!(
        validate_config(&serde_json::Value::Null)
            .unwrap_err()
            .to_string()
            .contains("missing")
    );
    assert!(s.work.exists());
}

#[test]
fn git_auth_env_scopes_an_http_extraheader_to_the_repo_host_via_env_not_argv_or_disk() {
    use base64::Engine as _;
    let env = git_auth_env("https://github.com/acme/repo.git", "tok");
    assert_eq!(
        env["GIT_CONFIG_KEY_0"],
        "http.https://github.com/.extraheader"
    );
    assert_eq!(
        env["GIT_CONFIG_VALUE_0"],
        format!(
            "AUTHORIZATION: basic {}",
            base64::engine::general_purpose::STANDARD.encode("x-access-token:tok")
        )
    );
    assert!(git_auth_env("/local/path.git", "tok").is_empty());
}

// ---------------------------------------------------------------- the real binary

const BIN: &str = env!("CARGO_BIN_EXE_ratchet-runner");

fn run_bin(env: &HashMap<String, String>) -> tokio::process::Child {
    tokio::process::Command::new(BIN)
        .env_clear()
        .envs(env)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn binary_exits_zero_and_writes_done_json_after_a_full_run() {
    let s = setup();
    let control = ssm_over(s.store.clone());
    control.start(&handle(), &s.config).await.unwrap();
    let child = run_bin(&s.env);
    let finished = Arc::new(AtomicBool::new(false));
    let waiter = {
        let finished = finished.clone();
        tokio::spawn(async move {
            let out = child.wait_with_output().await.unwrap();
            finished.store(true, Ordering::SeqCst);
            out
        })
    };
    engine_loop(&control, &finished).await;
    let out = waiter.await.unwrap();
    assert_eq!(
        out.status.code(),
        Some(0),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let done: serde_json::Value = serde_json::from_str(
        &s.store
            .get(&done_key(&runner_prefix("opt1", "cc")))
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(done["candidates"], json!(2));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn binary_sigterm_aborts_the_agent_and_exits_cleanly_recording_why() {
    let s = setup();
    let slow = s.work.join("slow.json");
    std::fs::write(
        &slow,
        serde_json::to_string(&AgentScript::new(
            "slow",
            vec![vec![ScriptStep::Sleep { ms: 600_000 }]],
        ))
        .unwrap(),
    )
    .unwrap();
    let control = ssm_over(s.store.clone());
    control.start(&handle(), &s.config).await.unwrap();
    let mut env = s.env.clone();
    env.insert(
        "RATCHET_MOCK_SCRIPT".into(),
        slow.to_string_lossy().into_owned(),
    );
    let mut child = run_bin(&env);
    let start = std::time::Instant::now();
    loop {
        if control
            .poll(&handle(), None)
            .await
            .unwrap()
            .events
            .iter()
            .any(|e| e.text.as_deref() == Some("agent turn started"))
        {
            break;
        }
        assert!(
            start.elapsed() < Duration::from_secs(15),
            "timed out waiting for the turn"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    // SAFETY: plain kill(2) of the child we spawned.
    unsafe { libc::kill(child.id().unwrap() as i32, libc::SIGTERM) };
    let status = child.wait().await.unwrap();
    assert_eq!(status.code(), Some(0));
    let done: serde_json::Value = serde_json::from_str(
        &s.store
            .get(&done_key(&runner_prefix("opt1", "cc")))
            .await
            .unwrap()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(done["reason"], json!("aborted"));
}

#[tokio::test]
async fn binary_exits_two_with_a_message_when_misconfigured() {
    let out = run_bin(&env_of(&[("PATH", &common::path_env())]))
        .wait_with_output()
        .await
        .unwrap();
    assert_eq!(out.status.code(), Some(2));
    assert!(String::from_utf8_lossy(&out.stderr).contains("RATCHET_OPTIMIZATION_ID is required"));
}
