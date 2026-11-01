mod common;

use ratchet_core::enums::AgentKind;
use ratchet_core::ports::{AgentAdapter, AgentEvent, AgentEventType, RunnerControl, RunnerHandle};
use std::collections::HashMap;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use common::{
    AutoOptions, Sandbox, auto_verdicts, base_config, env_of, make_sandbox, make_sandbox_branches,
    sh,
};

use ratchet_agents::{
    AgentScript, DetectOptions, HarnessAuth, HarnessInfo, HarnessKind, LocalAdapterOptions,
    LocalAgentControlOptions, LocalRunnerControl, LocalRunnerControlOptions, ScriptStep,
    ScriptedAgent, WorkdirMode, create_local_agent_control, detect_harnesses,
    local_adapter_factory, local_agent_problem, resolve_login_path,
};

fn handle(agent: &str) -> RunnerHandle {
    RunnerHandle {
        optimization_id: "opt1".into(),
        agent_id: agent.into(),
        instance_id: format!("local:{agent}"),
    }
}

/// Plays the engine for one agent until its loop ends.
async fn drive(control: &Arc<LocalRunnerControl>, h: &RunnerHandle) -> Vec<AgentEvent> {
    let answer = {
        let (control, h) = (control.clone(), h.clone());
        auto_verdicts(
            move |m| {
                let (control, h) = (control.clone(), h.clone());
                tokio::spawn(async move {
                    let _ = control.send(&h, &m).await;
                });
            },
            AutoOptions::default(),
        )
    };
    let finished = Arc::new(AtomicBool::new(false));
    {
        let (control, h, finished) = (control.clone(), h.clone(), finished.clone());
        tokio::spawn(async move {
            let _ = control.result(&h).await;
            finished.store(true, Ordering::SeqCst);
        });
    }
    let mut seen = Vec::new();
    let mut cursor: Option<String> = None;
    loop {
        let done = finished.load(Ordering::SeqCst);
        let r = control.poll(h, cursor.as_deref()).await.unwrap();
        cursor = Some(r.cursor);
        for e in r.events {
            answer(&e);
            seen.push(e);
        }
        if done {
            return seen;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn one_change(file: &str) -> AgentScript {
    AgentScript::new(
        "w",
        vec![vec![ScriptStep::Write {
            path: file.into(),
            content: format!("{file}\n"),
        }]],
    )
}

fn slow() -> AgentScript {
    AgentScript::new("s", vec![vec![ScriptStep::Sleep { ms: 60_000 }]])
}

fn worktree_control(
    root: PathBuf,
    scripts: HashMap<String, AgentScript>,
) -> Arc<LocalRunnerControl> {
    let mut options = LocalRunnerControlOptions::new(
        root,
        Arc::new(move |config, _| {
            Ok(Arc::new(ScriptedAgent::new(
                scripts[config.agent_id.as_str()].clone(),
            )) as Arc<dyn AgentAdapter>)
        }),
    );
    options.workdirs = WorkdirMode::Worktree;
    options.loop_tuning.max_idle_turns = Some(1);
    options.loop_tuning.retry_delay = Some(Duration::from_millis(1));
    options.stop_grace = Some(Duration::from_secs(2));
    Arc::new(LocalRunnerControl::new(options))
}

fn scripts(pairs: &[(&str, AgentScript)]) -> HashMap<String, AgentScript> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.clone()))
        .collect()
}

fn config_for(sb: &Sandbox, agent: &str) -> ratchet_core::ports::RunnerConfig {
    let (agent, branch) = (agent.to_string(), format!("ratchet/{agent}"));
    base_config(sb, move |c| {
        c["agentId"] = agent.into();
        c["branch"] = branch.into();
    })
}

async fn wait_for_turn(control: &LocalRunnerControl, h: &RunnerHandle) {
    let start = Instant::now();
    loop {
        if control
            .poll(h, None)
            .await
            .unwrap()
            .events
            .iter()
            .any(|e| e.text.as_deref() == Some("agent turn started"))
        {
            return;
        }
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timed out waiting for the turn"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worktree_mode_gives_each_agent_a_worktree_of_one_local_clone_with_its_own_identity_branch_and_refs()
 {
    let sb = make_sandbox_branches(&["ratchet/a", "ratchet/b"]);
    let root = sb.dir.join("agents");
    let control = worktree_control(
        root.clone(),
        scripts(&[("a", one_change("a.txt")), ("b", one_change("b.txt"))]),
    );
    let (ha, hb) = (handle("a"), handle("b"));
    control.start(&ha, &config_for(&sb, "a")).await.unwrap();
    control.start(&hb, &config_for(&sb, "b")).await.unwrap();

    let clone = root.join("opt1/clone");
    assert_eq!(control.workdir_of(&ha).unwrap(), root.join("opt1/a"));
    assert!(clone.join(".git").exists());
    // a worktree has a `.git` *file*, not a directory: it is not a second clone
    let gitfile = std::fs::read_to_string(control.workdir_of(&ha).unwrap().join(".git")).unwrap();
    assert!(
        gitfile.starts_with("gitdir: ") && gitfile.contains("worktrees/a"),
        "{gitfile}"
    );
    assert_eq!(sh(&clone, &["worktree", "list"]).lines().count(), 3);

    let (seen_a, _) = tokio::join!(drive(&control, &ha), drive(&control, &hb));
    assert_eq!(
        seen_a
            .iter()
            .filter(|e| e.event_type == AgentEventType::Commit)
            .count(),
        1
    );

    // both branches reached the remote with their own file, and no shared-ref clobbering
    assert_eq!(sh(&sb.remote, &["show", "ratchet/a:a.txt"]), "a.txt");
    assert_eq!(sh(&sb.remote, &["show", "ratchet/b:b.txt"]), "b.txt");
    // (the commit author itself can be overridden by GIT_AUTHOR_* in the caller's environment, so check the config git uses)
    assert_eq!(
        sh(&control.workdir_of(&ha).unwrap(), &["config", "user.name"]),
        "a (ratchet)"
    );
    assert_eq!(
        sh(&control.workdir_of(&hb).unwrap(), &["config", "user.email"]),
        "b@ratchet.local"
    );
    let refs = sh(
        &clone,
        &["for-each-ref", "--format=%(refname)", "refs/ratchet"],
    );
    assert!(
        refs.contains("refs/ratchet/a/accepted") && refs.contains("refs/ratchet/b/accepted"),
        "{refs}"
    );
    assert_eq!(
        sh(
            &control.workdir_of(&ha).unwrap(),
            &["config", "--worktree", "commit.gpgsign"]
        ),
        "false"
    );
    control.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worktree_mode_stop_removes_that_agents_worktree_and_nothing_else_release_removes_the_clone()
 {
    let sb = make_sandbox_branches(&["ratchet/a", "ratchet/b"]);
    let root = sb.dir.join("agents");
    let control = worktree_control(root.clone(), scripts(&[("a", slow()), ("b", slow())]));
    let (ha, hb) = (handle("a"), handle("b"));
    control.start(&ha, &config_for(&sb, "a")).await.unwrap();
    control.start(&hb, &config_for(&sb, "b")).await.unwrap();
    wait_for_turn(&control, &ha).await;

    control.stop(&ha).await.unwrap();
    assert!(!root.join("opt1/a").exists());
    assert!(root.join("opt1/b").exists());
    assert!(!sh(&root.join("opt1/clone"), &["worktree", "list"]).contains("/a "));

    control.release(&"opt1".into()).await.unwrap();
    assert!(!root.join("opt1").exists());
    // the remote is untouched
    let sha = sh(&sb.remote, &["rev-parse", "ratchet/a"]);
    assert!(sha.len() == 40 && sha.bytes().all(|b| b.is_ascii_hexdigit()));
    control.release(&"opt1".into()).await.unwrap(); // idempotent
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worktree_mode_a_start_that_cannot_clone_fails_cleanly_is_not_registered_and_can_be_retried()
 {
    let sb = make_sandbox();
    let control = worktree_control(
        sb.dir.join("agents"),
        scripts(&[("a", one_change("a.txt"))]),
    );
    let h = handle("a");
    let missing = sb.dir.join("missing.git").to_string_lossy().into_owned();
    let mut bad = config_for(&sb, "a");
    bad.repo_url = missing;
    let err = control.start(&h, &bad).await.unwrap_err();
    assert!(err.to_string().contains("clone"), "{err}");
    let unknown = control
        .send(&h, &ratchet_core::ports::RunnerMessage::Pause)
        .await
        .unwrap_err();
    assert!(unknown.to_string().contains("unknown runner"), "{unknown}");
    control.start(&h, &config_for(&sb, "a")).await.unwrap();
    control.stop_all().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn worktree_mode_a_second_run_reuses_the_existing_clone_and_replaces_a_stale_worktree() {
    let sb = make_sandbox();
    let root = sb.dir.join("agents");
    let first = worktree_control(root.clone(), scripts(&[("a", one_change("a.txt"))]));
    first
        .start(&handle("a"), &config_for(&sb, "a"))
        .await
        .unwrap();
    drive(&first, &handle("a")).await;
    // crash: no stop, no release. A new control (new process) starts the same agent again.
    let second = worktree_control(root, scripts(&[("a", one_change("a2.txt"))]));
    second
        .start(&handle("a"), &config_for(&sb, "a"))
        .await
        .unwrap();
    drive(&second, &handle("a")).await;
    assert_eq!(sh(&sb.remote, &["show", "ratchet/a:a2.txt"]), "a2.txt");
    second.release(&"opt1".into()).await.unwrap();
}

#[tokio::test]
async fn poll_reports_an_unknown_lost_agent_as_failed_once_so_the_engine_ends_it() {
    let sb = make_sandbox();
    let control = worktree_control(sb.dir.join("agents"), HashMap::new());
    let first = control.poll(&handle("ghost"), None).await.unwrap();
    assert_eq!(first.events.len(), 1);
    let e = &first.events[0];
    assert_eq!(e.event_type, AgentEventType::Status);
    assert_eq!(e.data_str("state"), Some("failed"));
    assert_eq!(e.data_str("reason"), Some("lost"));
    assert!(
        control
            .poll(&handle("ghost"), Some(&first.cursor))
            .await
            .unwrap()
            .events
            .is_empty()
    );
}

// ---------------------------------------------------------------- the real adapters against shim CLIs

const STREAM: [&str; 3] = [
    r#"{"type":"system","subtype":"init","session_id":"sess-1","model":"shim"}"#,
    r#"{"type":"assistant","message":{"content":[{"type":"text","text":"made a change"}]}}"#,
    r#"{"type":"result","subtype":"success","is_error":false,"result":"done","num_turns":1,"duration_ms":5,"total_cost_usd":0.02,"usage":{"input_tokens":10,"output_tokens":5}}"#,
];

fn make_executable(file: &Path, body: &str) {
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, body).unwrap();
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o755)).unwrap();
}

/// A `claude` stand-in: answers --version and `auth status`, and on a turn edits the worktree once and logs what it saw.
fn claude_shim(dir: &Path, seen_file: &Path) -> PathBuf {
    let file = dir.join("claude");
    let mut lines = vec![
        "#!/bin/sh".to_string(),
        "case \"$1\" in".to_string(),
        "  --version) echo \"2.1.4 (Claude Code)\"; exit 0;;".to_string(),
        "  auth) echo '{\"loggedIn\":true,\"authMethod\":\"claude.ai\"}'; exit 0;;".to_string(),
        "esac".to_string(),
        "cat >/dev/null".to_string(),
        format!(
            "printf 'cwd=%s\\nkey=%s\\npath=%s\\ngh=%s\\naws=%s\\nargs=%s\\n' \"$PWD\" \"$ANTHROPIC_API_KEY\" \"$PATH\" \"$GITHUB_TOKEN\" \"$AWS_SECRET_ACCESS_KEY\" \"$*\" >> {:?}",
            seen_file.to_string_lossy()
        ),
        "case \"$*\" in *--resume*) ;; *) mkdir -p src; echo \"shim edit\" >> src/app.txt;; esac"
            .to_string(),
    ];
    lines.extend(STREAM.iter().map(|l| format!("echo '{l}'")));
    lines.push(String::new());
    make_executable(&file, &lines.join("\n"));
    file
}

fn info(path: &Path, auth: HarnessAuth) -> HarnessInfo {
    HarnessInfo {
        kind: HarnessKind::ClaudeCode,
        installed: true,
        version: Some("2.1.4".into()),
        path: Some(path.to_string_lossy().into_owned()),
        auth,
        hint: String::new(),
    }
}

fn tuned(opts: &mut LocalAgentControlOptions) {
    opts.loop_tuning.max_idle_turns = Some(1);
    opts.loop_tuning.retry_delay = Some(Duration::from_millis(1));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_agents_run_claude_code_adapter_with_the_detected_binary_the_user_login_path_and_no_api_key()
 {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let seen = root.join("seen.txt");
    let bin = claude_shim(&root.join("bin"), &seen);
    let sb = make_sandbox();
    let mut opts = LocalAgentControlOptions {
        root_dir: root.join("agents"),
        adapters: LocalAdapterOptions {
            harnesses: vec![info(&bin, HarnessAuth::SignedIn)],
            login_path: format!("{}:/usr/bin:/bin", root.join("bin").display()),
            base_env: Some(env_of(&[
                ("HOME", &root.to_string_lossy()),
                ("PATH", "/stripped"),
                ("ANTHROPIC_API_KEY", "sk-ant-MUSTNOTPASS"),
                ("GITHUB_TOKEN", "ghp_MUSTNOTPASS"),
                ("AWS_SECRET_ACCESS_KEY", "aws-MUSTNOTPASS"),
            ])),
            ..LocalAdapterOptions::default()
        },
        ..LocalAgentControlOptions::default()
    };
    tuned(&mut opts);
    let control = Arc::new(create_local_agent_control(opts));
    let h = handle("cc");
    control
        .start(
            &h,
            &base_config(&sb, |c| {
                c["agentId"] = "cc".into();
                c["kind"] = "claude-code".into();
                c["branch"] = "ratchet/a".into();
            }),
        )
        .await
        .unwrap();
    let events = drive(&control, &h).await;

    assert!(
        events
            .iter()
            .any(|e| e.event_type == AgentEventType::Commit)
    );
    assert_eq!(
        events
            .iter()
            .find(|e| e.event_type == AgentEventType::Usage)
            .and_then(|e| e.usage)
            .and_then(|u| u.cost_usd),
        Some(0.02)
    );
    assert!(sh(&sb.remote, &["show", "ratchet/a:src/app.txt"]).contains("shim edit"));

    let log = std::fs::read_to_string(&seen).unwrap();
    assert!(
        log.contains(&format!("cwd={}", root.join("agents/opt1/cc").display())),
        "{log}"
    );
    assert!(
        log.contains("\nkey=\n"),
        "signed-in: the key in the environment is not passed\n{log}"
    );
    assert!(log.contains("\ngh=\n"));
    assert!(log.contains("\naws=\n"));
    assert!(log.contains(&format!(
        "path={}:/usr/bin:/bin",
        root.join("bin").display()
    )));
    assert!(!log.contains("MUSTNOTPASS"));
    assert!(log.contains("--output-format stream-json"));
    assert!(
        sh(&sb.remote, &["log", "-1", "--format=%B", "ratchet/a"])
            .contains("Ratchet-Candidate: true")
    );
    control.release(&"opt1".into()).await.unwrap();
    assert!(!root.join("agents/opt1").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_agents_pass_the_key_only_when_the_harness_reports_api_key_auth() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let seen = root.join("seen.txt");
    let bin = claude_shim(&root.join("bin"), &seen);
    let sb = make_sandbox();
    let mut opts = LocalAgentControlOptions {
        root_dir: root.join("agents"),
        adapters: LocalAdapterOptions {
            harnesses: vec![info(&bin, HarnessAuth::ApiKey)],
            login_path: "/usr/bin:/bin".into(),
            base_env: Some(env_of(&[
                ("HOME", &root.to_string_lossy()),
                ("ANTHROPIC_API_KEY", "sk-ant-OKTOPASS"),
            ])),
            ..LocalAdapterOptions::default()
        },
        ..LocalAgentControlOptions::default()
    };
    tuned(&mut opts);
    let control = Arc::new(create_local_agent_control(opts));
    let h = handle("cc");
    control
        .start(
            &h,
            &base_config(&sb, |c| {
                c["agentId"] = "cc".into();
                c["kind"] = "claude-code".into();
            }),
        )
        .await
        .unwrap();
    drive(&control, &h).await;
    assert!(
        std::fs::read_to_string(&seen)
            .unwrap()
            .contains("key=sk-ant-OKTOPASS")
    );
    control.release(&"opt1".into()).await.unwrap();
}

#[test]
fn local_agents_refuse_to_launch_a_harness_that_is_missing_or_signed_out_with_the_detection_hint() {
    let out = HarnessInfo {
        kind: HarnessKind::Codex,
        installed: true,
        version: Some("1".into()),
        path: Some("/x/codex".into()),
        auth: HarnessAuth::SignedOut,
        hint: "Run `codex login`.".into(),
    };
    let missing = HarnessInfo {
        kind: HarnessKind::ClaudeCode,
        installed: false,
        version: None,
        path: None,
        auth: HarnessAuth::SignedOut,
        hint: "Install Claude Code.".into(),
    };
    let both = [out.clone(), missing];
    assert_eq!(
        local_agent_problem(&both, HarnessKind::Codex).as_deref(),
        Some("Run `codex login`.")
    );
    assert_eq!(
        local_agent_problem(&both, HarnessKind::ClaudeCode).as_deref(),
        Some("Install Claude Code.")
    );
    assert!(
        local_agent_problem(&[], HarnessKind::Codex)
            .unwrap()
            .contains("not probed")
    );
    assert_eq!(
        local_agent_problem(
            &[HarnessInfo {
                auth: HarnessAuth::SignedIn,
                ..out
            }],
            HarnessKind::Codex
        ),
        None
    );

    let factory = local_adapter_factory(LocalAdapterOptions {
        harnesses: both.to_vec(),
        login_path: "/bin".into(),
        ..LocalAdapterOptions::default()
    });
    let codex_cfg = common::base_config_with("x", |c| c["kind"] = "codex".into());
    let err = factory(&codex_cfg).err().expect("codex is signed out");
    assert!(
        err.contains("cannot run codex") && err.contains("codex login"),
        "{err}"
    );
    let mock_cfg = common::base_config_with("x", |c| c["kind"] = "mock".into());
    assert!(
        factory(&mock_cfg)
            .err()
            .expect("no mock factory")
            .contains("mock agents")
    );
    assert_eq!(codex_cfg.kind, AgentKind::Codex);
}

// ---------------------------------------------------------------- detection against real processes

#[tokio::test]
async fn detection_finds_shims_through_the_login_shell_path_reads_version_and_auth() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let bin = root.join("bin");
    claude_shim(&bin, &root.join("seen.txt"));
    make_executable(
        &bin.join("codex"),
        "#!/bin/sh\ncase \"$1\" in --version) echo \"codex-cli 0.46.0\";; login) echo \"Logged in using ChatGPT\" >&2;; esac\n",
    );
    let env = env_of(&[
        ("PATH", &format!("{}:/usr/bin:/bin", bin.display())),
        ("HOME", &root.to_string_lossy()),
        ("SHELL", "/bin/sh"),
    ]);
    let base = DetectOptions {
        env: Some(env),
        home: Some(root.to_string_lossy().into_owned()),
        ..DetectOptions::default()
    };
    let login = resolve_login_path(&base).await;
    assert!(login.path.split(':').any(|p| p == bin.to_string_lossy()));
    let found = detect_harnesses(&DetectOptions {
        login_path: Some(login.path),
        ..base
    })
    .await;
    let (cc, cx) = (&found[0], &found[1]);
    assert_eq!(
        (cc.kind, cc.installed, cc.version.as_deref(), cc.auth),
        (
            HarnessKind::ClaudeCode,
            true,
            Some("2.1.4"),
            HarnessAuth::SignedIn
        )
    );
    assert_eq!(
        cc.path.as_deref(),
        Some(bin.join("claude").to_str().unwrap())
    );
    assert_eq!(
        (cx.kind, cx.installed, cx.version.as_deref(), cx.auth),
        (
            HarnessKind::Codex,
            true,
            Some("0.46.0"),
            HarnessAuth::SignedIn
        )
    );
    assert_eq!(
        cx.path.as_deref(),
        Some(bin.join("codex").to_str().unwrap())
    );
}

#[tokio::test]
async fn detection_a_missing_cli_is_not_installed_and_a_hanging_one_times_out() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().canonicalize().unwrap();
    let bin = root.join("bin");
    make_executable(&bin.join("codex"), "#!/bin/sh\nsleep 30\n");
    let t0 = Instant::now();
    let found = detect_harnesses(&DetectOptions {
        env: Some(env_of(&[
            ("PATH", &format!("{}:/usr/bin:/bin", bin.display())),
            ("HOME", &root.to_string_lossy()),
            ("SHELL", "/bin/sh"),
        ])),
        home: Some(root.to_string_lossy().into_owned()),
        timeout: Some(Duration::from_millis(300)),
        ..DetectOptions::default()
    })
    .await;
    assert!(t0.elapsed() < Duration::from_secs(5));
    let (cc, cx) = (&found[0], &found[1]);
    assert_eq!((cc.installed, cc.path.clone()), (false, None));
    assert_eq!(
        (cx.installed, cx.path.as_deref()),
        (false, Some(bin.join("codex").to_str().unwrap()))
    );
    assert!(cx.hint.contains("timed out"), "{}", cx.hint);
}
