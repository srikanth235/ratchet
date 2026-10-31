mod common;

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use common::{
    AutoOptions, FakeCompute, MemoryStore, auto_verdicts, base_config, base_config_with,
    make_sandbox_branches,
};
use ratchet_agents::domain::{
    AgentAdapter, AgentEvent, AgentEventType, ArtifactStore, ControlError, RemoteCommandResult,
    RunnerControl, RunnerHandle, RunnerMessage, StoreError,
};
use ratchet_agents::testing::{FakeProcessSpec, FakeSpawner};
use ratchet_agents::{
    AcceleratedClock, AgentScript, ArtifactEventSink, ArtifactInbox, AwsCliArtifactStore,
    EventSinkOptions, FsArtifactStore, LocalRunnerControl, LocalRunnerControlOptions,
    RunnerStopReason, ScriptStep, ScriptedAgent, SsmRunnerControl, SsmRunnerControlOptions,
    config_key, event_key, inbox_key, read_events, runner_prefix, seq_of,
};
use tokio_util::sync::CancellationToken;

fn handle() -> RunnerHandle {
    RunnerHandle {
        optimization_id: "opt1".into(),
        agent_id: "cc".into(),
        instance_id: "i-0abc".into(),
    }
}

fn ev(text: &str) -> AgentEvent {
    AgentEvent::new(1, AgentEventType::Status).with_text(text)
}

fn jsonl(events: &[&str]) -> String {
    events
        .iter()
        .map(|t| format!("{}\n", serde_json::to_string(&ev(t)).unwrap()))
        .collect()
}

#[test]
fn artifact_layout_uses_the_per_agent_prefix_iam_allows() {
    assert_eq!(runner_prefix("opt1", "cc"), "artifacts/opt1/cc/");
    assert_eq!(
        event_key("artifacts/opt1/cc/", 12),
        "artifacts/opt1/cc/events/00000012.jsonl"
    );
    assert_eq!(
        inbox_key("artifacts/opt1/cc/", 3),
        "artifacts/opt1/cc/inbox/00000003.json"
    );
    assert_eq!(seq_of("artifacts/opt1/cc/events/00000012.jsonl"), Some(12));
    assert_eq!(seq_of("artifacts/opt1/cc/config.json"), None);
}

// ---------------------------------------------------------------- SsmRunnerControl

fn ssm() -> (Arc<MemoryStore>, Arc<FakeCompute>, SsmRunnerControl) {
    let store = Arc::new(MemoryStore::default());
    let compute = Arc::new(FakeCompute::default());
    let mut runner_env = BTreeMap::new();
    runner_env.insert(
        "RATCHET_ARTIFACTS_BUCKET".to_string(),
        "my-bucket".to_string(),
    );
    runner_env.insert(
        "RATCHET_SECRET_REFS".to_string(),
        "ANTHROPIC_API_KEY=/ratchet/opt1/cc/ANTHROPIC_API_KEY".to_string(),
    );
    let control = SsmRunnerControl::new(SsmRunnerControlOptions {
        compute: compute.clone(),
        store: store.clone(),
        runner_command: None,
        runner_env,
        state_dir: None,
    });
    (store, compute, control)
}

#[tokio::test]
async fn ssm_start_writes_config_json_and_launches_ratchet_runner_detached_through_ssm() {
    let (store, compute, control) = ssm();
    let config = base_config_with("x", |c| c["agentId"] = "cc".into());
    control.start(&handle(), &config).await.unwrap();
    let stored: serde_json::Value =
        serde_json::from_str(&store.text(&config_key("artifacts/opt1/cc/")).unwrap()).unwrap();
    assert_eq!(
        (&stored["agentId"], &stored["branch"]),
        (&"cc".into(), &"ratchet/a".into())
    );
    let commands = compute.commands.lock().unwrap();
    assert_eq!(commands.len(), 1);
    let (instance_id, command) = &commands[0];
    assert_eq!(instance_id, "i-0abc");
    assert!(command.script.contains("setsid nohup env RATCHET_OPTIMIZATION_ID='opt1' RATCHET_AGENT_ID='cc' RATCHET_ARTIFACTS_BUCKET='my-bucket'"), "{}", command.script);
    assert!(command.script.contains("ratchet-runner"));
    assert!(command.script.contains("/var/lib/ratchet/opt1-cc.pid"));
    assert!(
        command
            .script
            .contains("RATCHET_SECRET_REFS='ANTHROPIC_API_KEY=/ratchet/opt1/cc/ANTHROPIC_API_KEY'")
    );
}

#[tokio::test]
async fn ssm_start_shell_quotes_values_and_surfaces_a_failed_launch() {
    let (_, compute, control) = ssm();
    *compute.next_result.lock().unwrap() = RemoteCommandResult {
        exit_code: 1,
        stdout: String::new(),
        stderr: "runner already running".into(),
    };
    let mut h = handle();
    h.agent_id = "o'brien".into();
    let err = control
        .start(&h, &base_config_with("x", |_| {}))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("runner already running"), "{err}");
    assert!(
        compute.commands.lock().unwrap()[0]
            .1
            .script
            .contains("RATCHET_AGENT_ID='o'\\''brien'")
    );
}

#[tokio::test]
async fn ssm_send_writes_numbered_inbox_objects_continuing_after_existing_ones() {
    let (store, _, control) = ssm();
    store
        .put(&inbox_key("artifacts/opt1/cc/", 4), b"{\"type\":\"pause\"}")
        .await
        .unwrap();
    control
        .send(
            &handle(),
            RunnerMessage::User {
                text: "hello".into(),
            },
        )
        .await
        .unwrap();
    control.send(&handle(), RunnerMessage::Stop).await.unwrap();
    let keys: Vec<String> = store
        .keys()
        .into_iter()
        .filter(|k| k.contains("/inbox/"))
        .collect();
    assert_eq!(
        keys,
        [
            "artifacts/opt1/cc/inbox/00000004.json",
            "artifacts/opt1/cc/inbox/00000005.json",
            "artifacts/opt1/cc/inbox/00000006.json"
        ]
    );
    let sent: serde_json::Value =
        serde_json::from_str(&store.text("artifacts/opt1/cc/inbox/00000005.json").unwrap())
            .unwrap();
    assert_eq!(sent, serde_json::json!({"type": "user", "text": "hello"}));
}

#[tokio::test]
async fn ssm_poll_returns_events_after_the_cursor_from_jsonl_batches_ignoring_torn_lines() {
    let (store, _, control) = ssm();
    let p = "artifacts/opt1/cc/";
    store
        .put(&event_key(p, 1), jsonl(&["a", "b"]).as_bytes())
        .await
        .unwrap();
    store
        .put(
            &event_key(p, 2),
            format!("{}{{\"at\":1,\"typ", jsonl(&["c"])).as_bytes(),
        )
        .await
        .unwrap();
    let first = control.poll(&handle(), None).await.unwrap();
    assert_eq!(common::texts(&first.events), ["a", "b", "c"]);
    assert_eq!(first.cursor, "2");
    assert!(
        control
            .poll(&handle(), Some(&first.cursor))
            .await
            .unwrap()
            .events
            .is_empty()
    );
    store
        .put(&event_key(p, 3), jsonl(&["d"]).as_bytes())
        .await
        .unwrap();
    assert_eq!(
        common::texts(
            &control
                .poll(&handle(), Some(&first.cursor))
                .await
                .unwrap()
                .events
        ),
        ["d"]
    );
    // other agents' events are not mixed in
    store
        .put(
            &event_key("artifacts/opt1/cx/", 1),
            jsonl(&["other"]).as_bytes(),
        )
        .await
        .unwrap();
    assert_eq!(control.poll(&handle(), None).await.unwrap().events.len(), 4);
}

#[tokio::test]
async fn ssm_stop_sends_a_durable_stop_message_plus_sigterm_to_the_runner_pid_and_tolerates_a_gone_instance()
 {
    let (store, compute, control) = ssm();
    control.stop(&handle()).await.unwrap();
    let sent: serde_json::Value =
        serde_json::from_str(&store.text("artifacts/opt1/cc/inbox/00000001.json").unwrap())
            .unwrap();
    assert_eq!(sent, serde_json::json!({"type": "stop"}));
    assert!(
        compute.commands.lock().unwrap()[0]
            .1
            .script
            .contains("kill -TERM")
    );
    *compute.fail.lock().unwrap() = true;
    control.stop(&handle()).await.unwrap();
}

#[tokio::test]
async fn ssm_round_trip_with_the_runner_side_classes_sink_to_poll_and_send_to_artifact_inbox() {
    let (store, _, control) = ssm();
    let prefix = "artifacts/opt1/cc/";
    let sink = ArtifactEventSink::new(
        store.clone(),
        prefix,
        EventSinkOptions {
            flush_interval: Some(Duration::from_secs(10)),
            max_buffered: None,
        },
    );
    sink.emit(ev("one"));
    sink.emit(ev("two"));
    sink.flush().await;
    sink.emit(ev("three"));
    sink.close().await;
    assert_eq!(
        store
            .keys()
            .iter()
            .filter(|k| k.contains("/events/"))
            .count(),
        2
    );
    assert_eq!(
        common::texts(&control.poll(&handle(), None).await.unwrap().events),
        ["one", "two", "three"]
    );

    let inbox = ArtifactInbox::new(store.clone(), prefix)
        .with_clock(Arc::new(AcceleratedClock::new(1000.0)))
        .with_poll_interval(Duration::from_millis(5));
    control
        .send(&handle(), RunnerMessage::User { text: "a".into() })
        .await
        .unwrap();
    control.send(&handle(), RunnerMessage::Pause).await.unwrap();
    let never = CancellationToken::new();
    use ratchet_agents::domain::RunnerInbox;
    assert_eq!(
        inbox.next(&never).await,
        Some(RunnerMessage::User { text: "a".into() })
    );
    assert_eq!(inbox.next(&never).await, Some(RunnerMessage::Pause));
    let cancel = CancellationToken::new();
    let control = Arc::new(control);
    let waiting = {
        let (inbox, cancel) = (&inbox, cancel.clone());
        let sender = {
            let control = control.clone();
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(30)).await;
                control
                    .send(&handle(), RunnerMessage::Resume)
                    .await
                    .unwrap();
            })
        };
        let m = inbox.next(&cancel).await;
        sender.await.unwrap();
        m
    };
    assert_eq!(waiting, Some(RunnerMessage::Resume));
    let idle = async { inbox.next(&cancel).await };
    cancel.cancel();
    assert_eq!(idle.await, None);
}

/// A store whose first `put` fails: an S3 503.
struct FlakyStore {
    inner: Arc<MemoryStore>,
    puts: Mutex<u32>,
}

#[async_trait]
impl ArtifactStore for FlakyStore {
    async fn put(&self, key: &str, body: &[u8]) -> Result<(), StoreError> {
        let first = {
            let mut n = self.puts.lock().unwrap();
            *n += 1;
            *n == 1
        };
        if first {
            return Err(StoreError("S3 503".into()));
        }
        self.inner.put(key, body).await
    }
    async fn get(&self, key: &str) -> Result<Option<String>, StoreError> {
        self.inner.get(key).await
    }
    async fn list(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        self.inner.list(prefix).await
    }
}

#[tokio::test]
async fn a_restarted_event_sink_continues_the_numbering_and_a_failed_put_is_retried_on_the_next_flush()
 {
    let store = Arc::new(MemoryStore::default());
    let p = "artifacts/o/a/";
    store.put(&event_key(p, 7), b"{}\n").await.unwrap();
    let flaky = Arc::new(FlakyStore {
        inner: store.clone(),
        puts: Mutex::new(0),
    });
    let sink = ArtifactEventSink::new(
        flaky,
        p,
        EventSinkOptions {
            flush_interval: Some(Duration::from_secs(10)),
            max_buffered: None,
        },
    );
    sink.emit(ev("x"));
    sink.flush().await;
    assert_eq!(sink.errors().len(), 1);
    sink.flush().await;
    sink.close().await;
    assert_eq!(store.keys(), [event_key(p, 7), event_key(p, 8)]);
    assert_eq!(
        common::texts(
            &read_events(store.as_ref(), p, Some("7"))
                .await
                .unwrap()
                .events
        ),
        ["x"]
    );
}

// ---------------------------------------------------------------- stores

#[tokio::test]
async fn aws_cli_store_put_get_list_through_s3_cp_and_s3api() {
    let fake = FakeSpawner::with_plan(|call, _| {
        let joined = call.args.join(" ");
        if call.args[0] == "s3api" {
            FakeProcessSpec::stdout(
                "[\"artifacts/o/a/events/00000001.jsonl\",\"artifacts/o/a/events/00000002.jsonl\"]\n",
            )
        } else if joined.contains("missing") {
            FakeProcessSpec::exit(1).with_stderr("An error occurred (404) when calling the HeadObject operation: Key \"missing\" does not exist")
        } else if call.args.get(3).map(String::as_str) == Some("-") {
            FakeProcessSpec::stdout("hello\n")
        } else {
            FakeProcessSpec::default()
        }
    });
    let store = AwsCliArtifactStore::new(
        "bkt",
        Some(Arc::new(fake.clone())),
        Some(common::env_of(&[("PATH", "/bin")])),
        Some("eu-west-1".into()),
    );
    store.put("k/1", b"body").await.unwrap();
    assert_eq!(
        fake.call(0).args,
        [
            "s3",
            "cp",
            "-",
            "s3://bkt/k/1",
            "--only-show-errors",
            "--region",
            "eu-west-1"
        ]
    );
    assert_eq!(fake.call(0).stdin, "body");
    assert_eq!(store.get("k/1").await.unwrap().as_deref(), Some("hello\n"));
    assert_eq!(store.get("missing").await.unwrap(), None);
    assert_eq!(store.list("artifacts/o/a/events/").await.unwrap().len(), 2);
    assert!(
        fake.calls()
            .last()
            .unwrap()
            .args
            .contains(&"list-objects-v2".to_string())
    );
}

#[tokio::test]
async fn aws_cli_store_list_returns_empty_when_the_prefix_is_empty_aws_prints_null() {
    let fake = FakeSpawner::new(FakeProcessSpec::stdout("null\n"));
    assert!(
        AwsCliArtifactStore::new("b", Some(Arc::new(fake)), Some(Default::default()), None)
            .list("x/")
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn fs_store_behaves_like_s3_for_put_get_list_by_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let store = FsArtifactStore::new(dir.path());
    store
        .put("artifacts/o/a/events/00000001.jsonl", b"x")
        .await
        .unwrap();
    store
        .put("artifacts/o/a/inbox/00000001.json", b"y")
        .await
        .unwrap();
    assert_eq!(
        store
            .get("artifacts/o/a/events/00000001.jsonl")
            .await
            .unwrap()
            .as_deref(),
        Some("x")
    );
    assert_eq!(store.get("nope").await.unwrap(), None);
    assert_eq!(
        store.list("artifacts/o/a/events/").await.unwrap(),
        ["artifacts/o/a/events/00000001.jsonl"]
    );
    assert_eq!(store.list("artifacts/").await.unwrap().len(), 2);
}

// ---------------------------------------------------------------- LocalRunnerControl

fn local_control(
    root: std::path::PathBuf,
    script: AgentScript,
    stop_grace: Option<Duration>,
) -> Arc<LocalRunnerControl> {
    let mut options = LocalRunnerControlOptions::new(
        root,
        Arc::new(move |_, _| {
            Ok(Arc::new(ScriptedAgent::new(script.clone())) as Arc<dyn AgentAdapter>)
        }),
    );
    options.loop_tuning.max_idle_turns = Some(1);
    options.loop_tuning.retry_delay = Some(Duration::from_millis(1));
    options.stop_grace = stop_grace;
    Arc::new(LocalRunnerControl::new(options))
}

fn local_handle(agent: &str) -> RunnerHandle {
    RunnerHandle {
        optimization_id: "opt1".into(),
        agent_id: agent.into(),
        instance_id: format!("local-{agent}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn local_control_runs_agents_in_separate_clones_with_an_in_memory_inbox_and_cursor_based_polling()
 {
    let sb = make_sandbox_branches(&["ratchet/a", "ratchet/b"]);
    let two = AgentScript::new(
        "l",
        vec![
            vec![
                ScriptStep::Write {
                    path: "one.txt".into(),
                    content: "1".into(),
                },
                ScriptStep::Usage {
                    input_tokens: 50_000,
                    output_tokens: 2_000,
                    cost_usd: None,
                },
            ],
            vec![ScriptStep::Write {
                path: "two.txt".into(),
                content: "2".into(),
            }],
        ],
    );
    let control = local_control(sb.dir.join("runners"), two, None);
    let (ha, hb) = (local_handle("a"), local_handle("b"));
    control
        .start(
            &ha,
            &base_config(&sb, |c| {
                c["agentId"] = "a".into();
                c["branch"] = "ratchet/a".into();
            }),
        )
        .await
        .unwrap();
    control
        .start(
            &hb,
            &base_config(&sb, |c| {
                c["agentId"] = "b".into();
                c["branch"] = "ratchet/b".into();
            }),
        )
        .await
        .unwrap();
    assert_ne!(
        control.workdir_of(&ha).unwrap(),
        control.workdir_of(&hb).unwrap()
    );
    let again = control
        .start(&ha, &base_config(&sb, |_| {}))
        .await
        .unwrap_err();
    assert!(again.to_string().contains("already started"), "{again}");

    let answer_for = |h: &RunnerHandle| {
        let (control, h) = (control.clone(), h.clone());
        auto_verdicts(
            move |m| {
                let (control, h) = (control.clone(), h.clone());
                tokio::spawn(async move { control.send(&h, m).await.unwrap() });
            },
            AutoOptions::default(),
        )
    };
    let (answer_a, answer_b) = (answer_for(&ha), answer_for(&hb));
    let finished = Arc::new(std::sync::atomic::AtomicBool::new(false));
    {
        let (control, finished, ha, hb) =
            (control.clone(), finished.clone(), ha.clone(), hb.clone());
        tokio::spawn(async move {
            let _ = control.result(&ha).await;
            let _ = control.result(&hb).await;
            finished.store(true, std::sync::atomic::Ordering::SeqCst);
        });
    }
    let mut seen: Vec<AgentEvent> = Vec::new();
    let (mut cursor_a, mut cursor_b): (Option<String>, Option<String>) = (None, None);
    loop {
        let done = finished.load(std::sync::atomic::Ordering::SeqCst);
        let r = control.poll(&ha, cursor_a.as_deref()).await.unwrap();
        cursor_a = Some(r.cursor);
        for e in r.events {
            answer_a(&e);
            seen.push(e);
        }
        let r = control.poll(&hb, cursor_b.as_deref()).await.unwrap();
        cursor_b = Some(r.cursor);
        r.events.iter().for_each(|e| answer_b(e));
        if done {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let (ra, rb) = (
        control.result(&ha).await.unwrap(),
        control.result(&hb).await.unwrap(),
    );
    assert_eq!((ra.reason, ra.candidates), (RunnerStopReason::Idle, 2));
    assert_eq!((rb.reason, rb.candidates), (RunnerStopReason::Idle, 2));
    // cursors never re-deliver
    assert_eq!(
        seen.iter()
            .filter(|e| e.kind == AgentEventType::Commit)
            .count(),
        2
    );
    assert!(
        control
            .poll(&ha, cursor_a.as_deref())
            .await
            .unwrap()
            .events
            .is_empty()
    );
    assert_eq!(
        control.poll(&ha, None).await.unwrap().events.len(),
        seen.len()
    ); // no cursor = from the start
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
async fn local_control_stop_ends_a_running_agent_promptly() {
    let sb = make_sandbox_branches(&["ratchet/a"]);
    let slow = AgentScript::new("s", vec![vec![ScriptStep::Sleep { ms: 60_000 }]]);
    let control = local_control(sb.dir.join("runners"), slow, None);
    let h = local_handle("a");
    control.start(&h, &base_config(&sb, |_| {})).await.unwrap();
    wait_for_turn(&control, &h).await;
    let t0 = Instant::now();
    control.stop(&h).await.unwrap();
    assert!(t0.elapsed() < Duration::from_secs(3));
    assert_eq!(
        control.result(&h).await.unwrap().reason,
        RunnerStopReason::Stopped
    );
    let unknown: ControlError = control
        .send(
            &RunnerHandle {
                agent_id: "zzz".into(),
                ..h.clone()
            },
            RunnerMessage::Pause,
        )
        .await
        .unwrap_err();
    assert!(unknown.to_string().contains("unknown runner"), "{unknown}");
}
