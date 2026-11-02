//! Live sessions (`Harness::open`) of the three CLIs against `tests/fake_cli`, which speaks each long-lived protocol from the S0
//! fixtures: multi-turn `send`, delta order, interrupt, resume after close, rate-limit and auth-expired classification, and the
//! spawn-per-turn fallback.

mod common;

use std::future::Future;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ratchet_agents::process::{ProcessHandle, SpawnOptions, Spawner, TokioSpawner};
use ratchet_agents::testing::{FakeProcessSpec, FakeSpawner};
use ratchet_agents::{
    ClaudeCodeAdapter, ClaudeCodeOptions, CliAgentOptions, CodexAdapter, CodexOptions, GrokAdapter,
    GrokOptions,
};
use ratchet_core::ports::{
    Access, AgentEvent, AgentEventType, Harness, McpServer, PortError, SessionSpec, SharedEventSink,
};
use serde_json::{Value, json};

use common::{collect, env_of, fixture};

type Events = Arc<Mutex<Vec<AgentEvent>>>;

const CLAUDE_SID: &str = "9c1e5d7a-0b2f-4c63-8a41-5d2f7e6b3c10";
const CODEX_THREAD: &str = "0199b3a0-5c1e-7a40-9d2e-6f1a2b3c4d5e";
const GROK_SESSION: &str = "01a2b3c4-d5e6-7f80-9a1b-2c3d4e5f6a7b";

/// Runs the fake CLI in place of the real binary: the n-th spawn plays the n-th `(mode, fixture)` (the last one repeats) and
/// everything the client writes to stdin is logged.
#[derive(Clone)]
struct Redirect {
    script: Vec<(&'static str, &'static str)>,
    calls: Arc<Mutex<Vec<Vec<String>>>>,
    log: PathBuf,
}

impl Spawner for Redirect {
    fn spawn(
        &self,
        _command: &str,
        args: &[String],
        options: &SpawnOptions,
    ) -> Box<dyn ProcessHandle> {
        let index = {
            let mut calls = self.calls.lock().unwrap();
            calls.push(args.to_vec());
            calls.len() - 1
        };
        let (mode, fixture) = self.script[index.min(self.script.len() - 1)];
        let mut options = options.clone();
        options.env.insert(
            "RATCHET_FAKE_CLI_LOG".into(),
            self.log.to_string_lossy().into(),
        );
        TokioSpawner.spawn(
            env!("CARGO_BIN_EXE_ratchet-fake-cli"),
            &[
                mode.to_string(),
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("tests/fixtures")
                    .join(fixture)
                    .to_string_lossy()
                    .into_owned(),
            ],
            &options,
        )
    }
}

impl Redirect {
    /// Every JSON line the client has written so far.
    fn requests(&self) -> Vec<Value> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .filter_map(|l| serde_json::from_str(l).ok())
            .collect()
    }

    fn methods(&self) -> Vec<String> {
        self.requests()
            .iter()
            .filter_map(|r| r["method"].as_str().map(str::to_string))
            .collect()
    }

    fn request(&self, method: &str) -> Value {
        self.requests()
            .into_iter()
            .find(|r| r["method"] == method)
            .unwrap_or_else(|| panic!("no {method} request"))
    }

    fn spawn_args(&self, n: usize) -> Vec<String> {
        self.calls.lock().unwrap()[n].clone()
    }

    fn spawns(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}

struct Rig {
    redirect: Redirect,
    workdir: tempfile::TempDir,
}

fn rig(script: &[(&'static str, &'static str)]) -> Rig {
    let workdir = tempfile::tempdir().unwrap();
    let redirect = Redirect {
        script: script.to_vec(),
        calls: Arc::default(),
        log: workdir.path().join("stdin.log"),
    };
    Rig { redirect, workdir }
}

fn cli(r: &Redirect, env: &[(&str, &str)]) -> CliAgentOptions {
    CliAgentOptions {
        spawner: Some(Arc::new(r.clone())),
        env: Some(env_of(env)),
        kill_grace: Some(Duration::from_millis(50)),
        ..CliAgentOptions::default()
    }
}

fn claude(r: &Redirect) -> ClaudeCodeAdapter {
    ClaudeCodeAdapter::new(ClaudeCodeOptions {
        cli: cli(r, &[("PATH", "/bin")]),
        allow_subscription_auth: true,
        ..ClaudeCodeOptions::default()
    })
}

fn codex(r: &Redirect) -> CodexAdapter {
    CodexAdapter::new(CodexOptions {
        cli: cli(r, &[("PATH", "/bin")]),
        allow_subscription_auth: true,
        ..CodexOptions::default()
    })
}

fn grok(r: &Redirect) -> GrokAdapter {
    GrokAdapter::new(GrokOptions {
        cli: cli(r, &[("PATH", "/bin"), ("XAI_API_KEY", "xai-k")]),
        ..GrokOptions::default()
    })
}

fn chat_spec(rig: &Rig) -> SessionSpec {
    SessionSpec {
        workdir: rig.workdir.path().to_string_lossy().into_owned(),
        access: Access::ReadOnly,
        mcp: vec![McpServer {
            name: "ratchet".into(),
            command: "/usr/local/bin/ratchetd".into(),
            args: vec!["mcp".into()],
            env: [("TOKEN".to_string(), "t".to_string())].into(),
        }],
        system_append: Some("Ratchet's role prompt".into()),
        ..SessionSpec::default()
    }
}

fn sink() -> (Events, SharedEventSink) {
    let (events, sink) = collect();
    (events, Arc::new(sink))
}

async fn within<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(20), f)
        .await
        .expect("finished in time")
}

fn of_type(events: &Events, t: AgentEventType) -> Vec<AgentEvent> {
    events
        .lock()
        .unwrap()
        .iter()
        .filter(|e| e.event_type == t)
        .cloned()
        .collect()
}

fn texts(events: &[AgentEvent]) -> Vec<String> {
    events.iter().filter_map(|e| e.text.clone()).collect()
}

fn kinds(events: &Events) -> Vec<AgentEventType> {
    events
        .lock()
        .unwrap()
        .iter()
        .map(|e| e.event_type)
        .collect()
}

/// Wait until `n` events of type `t` have arrived.
async fn wait_for(events: &Events, t: AgentEventType, n: usize) {
    within(async {
        while of_type(events, t).len() < n {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let at = args.iter().position(|a| a == name)?;
    args.get(at + 1).map(String::as_str)
}

use AgentEventType::{AuthExpired, Delta, Message, RateLimited, Status, Usage};

// ---------------------------------------------------------------------------------------------------- Claude Code

#[tokio::test]
async fn claude_live_session_answers_two_turns_in_one_process_with_deltas_before_each_message() {
    let rig = rig(&[("claude-stream-json", "claude-2.1.316-mock-session.jsonl")]);
    let (events, sink) = sink();
    let spec = chat_spec(&rig);
    let session = within(claude(&rig.redirect).open(&spec, sink))
        .await
        .unwrap();
    assert!(
        events.lock().unwrap().is_empty(),
        "nothing before the first message"
    );

    within(session.send("hello")).await.unwrap();
    assert_eq!(kinds(&events), [Status, Delta, Delta, Message, Usage]);
    within(session.send("and now?")).await.unwrap();

    assert_eq!(
        texts(&of_type(&events, Delta)),
        [
            "Hello. ",
            "Ask me about your optimizations.",
            "The ",
            "newest ",
            "one is ",
            "paused."
        ]
    );
    assert_eq!(
        texts(&of_type(&events, Message)),
        [
            "Hello. Ask me about your optimizations.",
            "The newest one is paused."
        ]
    );
    assert_eq!(of_type(&events, Usage).len(), 2);
    assert_eq!(session.session_id().as_deref(), Some(CLAUDE_SID));

    // one process, two user lines in the stream-json input format
    assert_eq!(rig.redirect.spawns(), 1);
    let users = rig.redirect.requests();
    assert_eq!(users.len(), 2);
    assert_eq!(users[0]["type"], "user");
    assert_eq!(
        users[0]["message"],
        json!({"role": "user", "content": "hello"})
    );
    assert_eq!(users[1]["message"]["content"], "and now?");

    let args = rig.redirect.spawn_args(0);
    assert_eq!(flag(&args, "--input-format"), Some("stream-json"));
    assert!(args.contains(&"--include-partial-messages".to_string()));
    assert!(args.contains(&"--strict-mcp-config".to_string()));
    assert_eq!(
        flag(&args, "--append-system-prompt"),
        Some("Ratchet's role prompt")
    );
    assert_eq!(flag(&args, "--permission-mode"), Some("dontAsk"));
    within(session.close()).await;
}

#[tokio::test]
async fn claude_live_session_sends_the_prompt_given_at_open_as_the_first_message() {
    let rig = rig(&[("claude-stream-json", "claude-2.1.316-mock-session.jsonl")]);
    let (events, sink) = sink();
    let mut spec = chat_spec(&rig);
    spec.prompt = "hello".into();
    let session = within(claude(&rig.redirect).open(&spec, sink))
        .await
        .unwrap();
    assert_eq!(of_type(&events, Message).len(), 1);
    assert_eq!(rig.redirect.requests()[0]["message"]["content"], "hello");
    within(session.close()).await;
}

#[tokio::test]
async fn claude_live_interrupt_ends_the_turn_without_an_error_and_the_next_turn_works() {
    let rig = rig(&[("claude-stream-json", "claude-2.1.316-mock-interrupt.jsonl")]);
    let (events, sink) = sink();
    let session = within(claude(&rig.redirect).open(&chat_spec(&rig), sink))
        .await
        .unwrap();
    let (sent, ()) = within(async {
        tokio::join!(session.send("count the runs"), async {
            wait_for(&events, Delta, 2).await;
            session.interrupt().await.unwrap();
        })
    })
    .await;
    sent.unwrap();
    assert_eq!(texts(&of_type(&events, Delta)), ["Counting ", "the runs"]);
    assert!(
        of_type(&events, AgentEventType::Error).is_empty(),
        "an interrupted turn is not an error"
    );
    let control = rig
        .redirect
        .requests()
        .into_iter()
        .find(|r| r["type"] == "control_request")
        .unwrap();
    assert_eq!(control["request"]["subtype"], "interrupt");

    within(session.send("go on")).await.unwrap();
    assert_eq!(
        texts(&of_type(&events, Message)),
        ["Back again."],
        "the session survived the interrupt"
    );
    // interrupting while idle does nothing
    session.interrupt().await.unwrap();
    within(session.close()).await;
}

#[tokio::test]
async fn claude_live_resume_after_close_continues_the_cli_session() {
    let rig = rig(&[("claude-stream-json", "claude-2.1.316-mock-session.jsonl")]);
    let (_, sink1) = sink();
    let first = within(claude(&rig.redirect).open(&chat_spec(&rig), sink1))
        .await
        .unwrap();
    within(first.send("hello")).await.unwrap();
    let id = first.session_id().unwrap();
    within(first.close()).await;

    let (events, sink2) = sink();
    let mut spec = chat_spec(&rig);
    spec.resume = Some(id.clone());
    let second = within(claude(&rig.redirect).open(&spec, sink2))
        .await
        .unwrap();
    assert_eq!(
        second.session_id().as_deref(),
        Some(id.as_str()),
        "known before the first message"
    );
    assert_eq!(rig.redirect.spawns(), 2);
    assert_eq!(
        flag(&rig.redirect.spawn_args(1), "--resume"),
        Some(CLAUDE_SID)
    );
    within(second.send("welcome back")).await.unwrap();
    assert_eq!(of_type(&events, Message).len(), 1);
    within(second.close()).await;
}

#[tokio::test]
async fn claude_live_rate_limit_and_auth_expired_fail_the_turn_and_say_why() {
    for (fixture, kind, resets) in [
        (
            "claude-2.1.316-mock-rate-limit.jsonl",
            RateLimited,
            Some(1793614200),
        ),
        ("claude-2.1.316-mock-auth-expired.jsonl", AuthExpired, None),
    ] {
        let rig = rig(&[("claude-stream-json", fixture)]);
        let (events, sink) = sink();
        let session = within(claude(&rig.redirect).open(&chat_spec(&rig), sink))
            .await
            .unwrap();
        let err = within(session.send("hi")).await.unwrap_err();
        assert!(
            matches!(err, PortError::AgentFailed { .. }),
            "{fixture}: {err:?}"
        );
        let classified = of_type(&events, kind);
        assert_eq!(classified.len(), 1, "{fixture}");
        if let Some(at) = resets {
            assert_eq!(classified[0].data.as_ref().unwrap()["resetsAt"], at);
        }
        within(session.close()).await;
    }
}

// ---------------------------------------------------------------------------------------------------- Codex

#[tokio::test]
async fn codex_app_server_session_runs_the_handshake_then_one_turn_start_per_message() {
    let rig = rig(&[("codex-app-server", "codex-0.166.0-mock-session.jsonl")]);
    let (events, sink) = sink();
    let session = within(codex(&rig.redirect).open(&chat_spec(&rig), sink))
        .await
        .unwrap();
    assert_eq!(session.session_id().as_deref(), Some(CODEX_THREAD));
    within(session.send("hello")).await.unwrap();
    within(session.send("and now?")).await.unwrap();

    assert_eq!(
        rig.redirect.methods(),
        [
            "initialize",
            "initialized",
            "thread/start",
            "turn/start",
            "turn/start"
        ]
    );
    let thread = rig.redirect.request("thread/start");
    assert_eq!(thread["params"]["sandbox"], "read-only");
    assert_eq!(
        thread["params"]["approvalPolicy"], "never",
        "nothing ever asks"
    );
    assert_eq!(
        thread["params"]["cwd"],
        rig.workdir.path().to_string_lossy().as_ref()
    );
    let turns: Vec<_> = rig
        .redirect
        .requests()
        .into_iter()
        .filter(|r| r["method"] == "turn/start")
        .collect();
    assert_eq!(turns[0]["params"]["threadId"], CODEX_THREAD);
    assert_eq!(
        turns[0]["params"]["input"],
        json!([{"type": "text", "text": "hello"}])
    );
    assert_eq!(turns[1]["params"]["input"][0]["text"], "and now?");

    // deltas in order, then the whole message and the usage of each turn
    let order: Vec<_> = kinds(&events);
    assert_eq!(
        order,
        [
            Status, Delta, Delta, Message, Usage, Delta, Delta, Message, Usage
        ]
    );
    assert_eq!(
        texts(&of_type(&events, Delta)),
        [
            "Hello. ",
            "Ask me about your optimizations.",
            "The newest one ",
            "is paused."
        ]
    );

    // the CLI's own -c values go before the subcommand: MCP, instructions, sandbox
    let args = rig.redirect.spawn_args(0);
    assert_eq!(args.last().map(String::as_str), Some("app-server"));
    assert!(args.contains(&"sandbox_mode=\"read-only\"".to_string()));
    assert!(
        args.iter()
            .any(|a| a.starts_with("mcp_servers.ratchet.command="))
    );
    assert!(
        args.iter()
            .any(|a| a.starts_with("developer_instructions="))
    );
    within(session.close()).await;
}

#[tokio::test]
async fn codex_app_server_interrupt_sends_turn_interrupt_with_the_turn_id() {
    let rig = rig(&[(
        "codex-app-server",
        "codex-0.166.0-mock-app-server-interrupt.jsonl",
    )]);
    let (events, sink) = sink();
    let session = within(codex(&rig.redirect).open(&chat_spec(&rig), sink))
        .await
        .unwrap();
    let (sent, ()) = within(async {
        tokio::join!(session.send("count"), async {
            wait_for(&events, Delta, 2).await;
            session.interrupt().await.unwrap();
        })
    })
    .await;
    sent.unwrap();
    let interrupt = rig.redirect.request("turn/interrupt");
    assert_eq!(
        interrupt["params"],
        json!({"threadId": CODEX_THREAD, "turnId": "turn_1"})
    );
    assert!(of_type(&events, AgentEventType::Error).is_empty());
    within(session.send("go on")).await.unwrap();
    assert_eq!(texts(&of_type(&events, Message)), ["Back again."]);
    within(session.close()).await;
}

#[tokio::test]
async fn codex_app_server_resume_uses_thread_resume_with_the_saved_id() {
    let rig = rig(&[("codex-app-server", "codex-0.166.0-mock-session.jsonl")]);
    let (_, sink) = sink();
    let mut spec = chat_spec(&rig);
    spec.resume = Some(CODEX_THREAD.into());
    spec.model = Some("gpt-5".into());
    let session = within(codex(&rig.redirect).open(&spec, sink))
        .await
        .unwrap();
    let methods = rig.redirect.methods();
    assert!(
        methods.contains(&"thread/resume".to_string())
            && !methods.contains(&"thread/start".to_string())
    );
    let resume = rig.redirect.request("thread/resume");
    assert_eq!(resume["params"]["threadId"], CODEX_THREAD);
    assert_eq!(resume["params"]["model"], "gpt-5");
    assert_eq!(session.session_id().as_deref(), Some(CODEX_THREAD));
    within(session.close()).await;
}

#[tokio::test]
async fn codex_app_server_rate_limit_and_auth_expired_fail_the_turn_and_say_why() {
    for (fixture, kind, resets) in [
        (
            "codex-0.166.0-mock-app-server-rate-limit.jsonl",
            RateLimited,
            Some(1793614200),
        ),
        (
            "codex-0.166.0-mock-app-server-auth-expired.jsonl",
            AuthExpired,
            None,
        ),
    ] {
        let rig = rig(&[("codex-app-server", fixture)]);
        let (events, sink) = sink();
        let session = within(codex(&rig.redirect).open(&chat_spec(&rig), sink))
            .await
            .unwrap();
        let err = within(session.send("hi")).await.unwrap_err();
        assert!(matches!(err, PortError::AgentFailed { .. }), "{fixture}");
        let classified = of_type(&events, kind);
        assert_eq!(classified.len(), 1, "{fixture}");
        if let Some(at) = resets {
            assert_eq!(classified[0].data.as_ref().unwrap()["resetsAt"], at);
        }
        within(session.close()).await;
    }
}

#[tokio::test]
async fn codex_app_server_mcp_tool_call_is_streamed_as_tool_events() {
    let rig = rig(&[(
        "codex-app-server",
        "codex-0.166.0-mock-app-server-mcp-tool.jsonl",
    )]);
    let (events, sink) = sink();
    let session = within(codex(&rig.redirect).open(&chat_spec(&rig), sink))
        .await
        .unwrap();
    within(session.send("list them")).await.unwrap();
    let tools = of_type(&events, AgentEventType::Tool);
    assert_eq!(tools[0].text.as_deref(), Some("ratchet.list_optimizations"));
    assert_eq!(tools.len(), 4);
    within(session.close()).await;
}

// ---------------------------------------------------------------------------------------------------- Grok

#[tokio::test]
async fn grok_acp_session_authenticates_creates_the_session_with_mcp_and_streams_two_prompts() {
    let rig = rig(&[("grok-acp", "grok-1.0.70-mock-session.jsonl")]);
    let (events, sink) = sink();
    let session = within(grok(&rig.redirect).open(&chat_spec(&rig), sink))
        .await
        .unwrap();
    assert_eq!(session.session_id().as_deref(), Some(GROK_SESSION));
    within(session.send("hello")).await.unwrap();
    within(session.send("and now?")).await.unwrap();

    assert_eq!(
        rig.redirect.methods(),
        [
            "initialize",
            "authenticate",
            "session/new",
            "session/prompt",
            "session/prompt"
        ]
    );
    assert_eq!(rig.redirect.request("initialize")["jsonrpc"], "2.0");
    let auth = rig.redirect.request("authenticate");
    assert_eq!(auth["params"]["methodId"], "xai.api_key");
    assert_eq!(auth["params"]["_meta"]["headless"], true);
    let new = rig.redirect.request("session/new");
    assert_eq!(
        new["params"]["cwd"],
        rig.workdir.path().to_string_lossy().as_ref()
    );
    assert_eq!(
        new["params"]["mcpServers"],
        json!([{
            "name": "ratchet",
            "command": "/usr/local/bin/ratchetd",
            "args": ["mcp"],
            "env": [{"name": "TOKEN", "value": "t"}],
        }])
    );
    assert_eq!(new["params"]["_meta"]["rules"], "Ratchet's role prompt");
    let prompt = rig.redirect.request("session/prompt");
    assert_eq!(prompt["params"]["sessionId"], GROK_SESSION);
    assert_eq!(
        prompt["params"]["prompt"],
        json!([{"type": "text", "text": "hello"}])
    );

    assert_eq!(
        kinds(&events),
        [Delta, Delta, Message, Delta, Delta, Message]
    );
    assert_eq!(
        texts(&of_type(&events, Message)),
        [
            "Hello. Ask me about your optimizations.",
            "The newest one is paused."
        ]
    );

    // read-only in agent mode is the tool allowlist: the sandbox is refused there
    let args = rig.redirect.spawn_args(0);
    assert_eq!(args[..2], ["agent", "stdio"]);
    assert!(flag(&args, "--tools").is_some() && flag(&args, "--disallowed-tools").is_some());
    assert!(!args.contains(&"--sandbox".to_string()));
    within(session.close()).await;
}

#[tokio::test]
async fn grok_acp_interrupt_sends_session_cancel_and_the_cancelled_prompt_ends_the_turn() {
    let rig = rig(&[("grok-acp", "grok-1.0.70-mock-acp-interrupt.jsonl")]);
    let (events, sink) = sink();
    let session = within(grok(&rig.redirect).open(&chat_spec(&rig), sink))
        .await
        .unwrap();
    let (sent, ()) = within(async {
        tokio::join!(session.send("count"), async {
            wait_for(&events, Delta, 2).await;
            session.interrupt().await.unwrap();
        })
    })
    .await;
    sent.unwrap();
    let cancel = rig.redirect.request("session/cancel");
    assert_eq!(cancel["params"]["sessionId"], GROK_SESSION);
    assert!(cancel.get("id").is_none(), "ACP cancel is a notification");
    assert!(of_type(&events, AgentEventType::Error).is_empty());
    within(session.send("go on")).await.unwrap();
    let messages = texts(&of_type(&events, Message));
    assert_eq!(messages.last().map(String::as_str), Some("Back again."));
    within(session.close()).await;
}

#[tokio::test]
async fn grok_acp_resume_loads_the_saved_session() {
    let rig = rig(&[("grok-acp", "grok-1.0.70-mock-session.jsonl")]);
    let (_, sink) = sink();
    let mut spec = chat_spec(&rig);
    spec.resume = Some(GROK_SESSION.into());
    spec.model = Some("grok-4".into());
    let session = within(grok(&rig.redirect).open(&spec, sink)).await.unwrap();
    let load = rig.redirect.request("session/load");
    assert_eq!(load["params"]["sessionId"], GROK_SESSION);
    assert!(!rig.redirect.methods().contains(&"session/new".to_string()));
    assert_eq!(session.session_id().as_deref(), Some(GROK_SESSION));
    within(session.close()).await;
}

#[tokio::test]
async fn grok_acp_rate_limit_and_auth_expired_fail_the_prompt_and_say_why() {
    for (fixture, kind) in [
        ("grok-1.0.70-mock-acp-rate-limit.jsonl", RateLimited),
        ("grok-1.0.70-mock-acp-auth-expired.jsonl", AuthExpired),
    ] {
        let rig = rig(&[("grok-acp", fixture)]);
        let (events, sink) = sink();
        let session = within(grok(&rig.redirect).open(&chat_spec(&rig), sink))
            .await
            .unwrap();
        let err = within(session.send("hi")).await.unwrap_err();
        assert!(matches!(err, PortError::AgentFailed { .. }), "{fixture}");
        assert_eq!(of_type(&events, kind).len(), 1, "{fixture}");
        within(session.close()).await;
    }
}

#[tokio::test]
async fn grok_acp_mcp_tool_call_is_streamed_as_tool_events() {
    let rig = rig(&[("grok-acp", "grok-1.0.70-mock-acp-mcp-tool.jsonl")]);
    let (events, sink) = sink();
    let session = within(grok(&rig.redirect).open(&chat_spec(&rig), sink))
        .await
        .unwrap();
    within(session.send("list them")).await.unwrap();
    assert_eq!(of_type(&events, AgentEventType::Tool).len(), 4);
    assert_eq!(of_type(&events, Usage).len(), 1);
    within(session.close()).await;
}

// ---------------------------------------------------------------------------------------------------- fallback

/// One-shot playback for the fallback tests: the first run replays `first`, later ones `later`.
fn fallback_spawner(first: &str, later: &str) -> FakeSpawner {
    let (first, later) = (fixture(first), fixture(later));
    FakeSpawner::with_plan(move |_, index| {
        FakeProcessSpec::stdout(if index == 0 {
            first.clone()
        } else {
            later.clone()
        })
    })
}

/// A spawner whose first process is a binary that does not exist (the live attempt), then `fake`.
#[derive(Clone)]
struct LiveFails {
    fake: FakeSpawner,
    spawned: Arc<std::sync::atomic::AtomicUsize>,
}

impl Spawner for LiveFails {
    fn spawn(
        &self,
        command: &str,
        args: &[String],
        options: &SpawnOptions,
    ) -> Box<dyn ProcessHandle> {
        if self
            .spawned
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst)
            == 0
        {
            TokioSpawner.spawn("/nonexistent/ratchet-cli", args, options)
        } else {
            self.fake.spawn(command, args, options)
        }
    }
}

fn one_shot_cli(spawner: Arc<dyn Spawner>, disable_live: bool) -> CliAgentOptions {
    CliAgentOptions {
        spawner: Some(spawner),
        env: Some(env_of(&[("PATH", "/bin"), ("XAI_API_KEY", "k")])),
        kill_grace: Some(Duration::from_millis(20)),
        disable_live_session: disable_live,
        ..CliAgentOptions::default()
    }
}

fn plain_spec() -> SessionSpec {
    SessionSpec {
        workdir: std::env::temp_dir().to_string_lossy().into_owned(),
        ..SessionSpec::default()
    }
}

#[tokio::test]
async fn without_live_sessions_each_message_is_a_start_then_a_resume() {
    let fake = fallback_spawner("claude-success.jsonl", "claude-resume.jsonl");
    let adapter = ClaudeCodeAdapter::new(ClaudeCodeOptions {
        cli: one_shot_cli(Arc::new(fake.clone()), true),
        allow_subscription_auth: true,
        ..ClaudeCodeOptions::default()
    });
    assert!(!adapter.capabilities().live_session);
    let (events, sink) = sink();
    let mut spec = plain_spec();
    spec.prompt = "hello".into();
    let session = within(adapter.open(&spec, sink)).await.unwrap();
    assert_eq!(
        fake.call_count(),
        1,
        "the prompt given at open ran the first turn"
    );
    assert_eq!(
        session.session_id().as_deref(),
        Some("5f1c1b8e-2d7a-4a37-9c0e-7b6f3b1d0a11")
    );
    within(session.send("verdict: verified")).await.unwrap();

    assert!(flag(&fake.call(0).args, "--resume").is_none());
    assert_eq!(
        flag(&fake.call(1).args, "--resume"),
        Some("5f1c1b8e-2d7a-4a37-9c0e-7b6f3b1d0a11")
    );
    assert_eq!(fake.call(1).stdin, "verdict: verified");
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.event_type == Status
                && e.text.as_deref().unwrap().contains("no live session"))
    );
    within(session.close()).await;
}

#[tokio::test]
async fn a_live_session_that_cannot_open_falls_back_for_every_cli() {
    type Build = Box<dyn Fn(CliAgentOptions) -> Box<dyn Harness>>;
    let cases: Vec<(&str, &str, &str, Build)> = vec![
        (
            "claude",
            "claude-success.jsonl",
            "claude-resume.jsonl",
            Box::new(|cli| {
                Box::new(ClaudeCodeAdapter::new(ClaudeCodeOptions {
                    cli,
                    allow_subscription_auth: true,
                    ..ClaudeCodeOptions::default()
                }))
            }),
        ),
        (
            "codex",
            "codex-success.jsonl",
            "codex-resume.jsonl",
            Box::new(|cli| {
                Box::new(CodexAdapter::new(CodexOptions {
                    cli,
                    allow_subscription_auth: true,
                    ..CodexOptions::default()
                }))
            }),
        ),
        (
            "grok",
            "grok-success.jsonl",
            "grok-resume.jsonl",
            Box::new(|cli| {
                Box::new(GrokAdapter::new(GrokOptions {
                    cli,
                    ..GrokOptions::default()
                }))
            }),
        ),
    ];
    for (name, first, later, build) in cases {
        // the live attempt cannot start; the per-turn runs after it replay `first` then `later`
        let fake = fallback_spawner(first, later);
        let spawner = LiveFails {
            fake: fake.clone(),
            spawned: Arc::default(),
        };
        let harness = build(one_shot_cli(Arc::new(spawner), false));
        assert!(harness.capabilities().live_session, "{name}");
        let (events, sink) = sink();
        let session = within(harness.open(&plain_spec(), sink))
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        within(session.send("hello"))
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        within(session.send("again"))
            .await
            .unwrap_or_else(|e| panic!("{name}: {e}"));
        assert_eq!(fake.call_count(), 2, "{name}: start, resume");
        let sid = session.session_id().unwrap_or_default();
        assert!(!sid.is_empty(), "{name}");
        assert!(
            fake.call(1).args.contains(&sid),
            "{name}: the second turn resumes {sid}: {:?}",
            fake.call(1).args
        );
        assert!(
            events.lock().unwrap().iter().any(|e| e.event_type == Status
                && e.text.as_deref().unwrap().contains("no live session")),
            "{name}"
        );
        within(session.close()).await;
    }
}

#[tokio::test]
async fn spawn_per_turn_interrupt_stops_the_running_process_and_send_returns_ok() {
    let fake = FakeSpawner::new(FakeProcessSpec::hang());
    let adapter = ClaudeCodeAdapter::new(ClaudeCodeOptions {
        cli: one_shot_cli(Arc::new(fake.clone()), true),
        allow_subscription_auth: true,
        ..ClaudeCodeOptions::default()
    });
    let (_, sink) = sink();
    let session = within(adapter.open(&plain_spec(), sink)).await.unwrap();
    let (sent, ()) = within(async {
        tokio::join!(session.send("long task"), async {
            while fake.call_count() == 0 {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
            session.interrupt().await.unwrap();
        })
    })
    .await;
    sent.unwrap();
    assert!(
        !fake.call(0).signals.is_empty(),
        "the process group was signalled"
    );
}

#[tokio::test]
async fn spawn_per_turn_failures_keep_their_classified_events() {
    let fake = FakeSpawner::new(FakeProcessSpec::stdout(fixture(
        "claude-2.1.316-mock-rate-limit.jsonl",
    )));
    let adapter = ClaudeCodeAdapter::new(ClaudeCodeOptions {
        cli: one_shot_cli(Arc::new(fake.clone()), true),
        allow_subscription_auth: true,
        ..ClaudeCodeOptions::default()
    });
    let (events, sink) = sink();
    let session = within(adapter.open(&plain_spec(), sink)).await.unwrap();
    let err = within(session.send("hi")).await.unwrap_err();
    assert!(matches!(err, PortError::AgentFailed { .. }));
    assert_eq!(of_type(&events, RateLimited).len(), 1);
    // a closed session refuses further messages
    let id = session.session_id();
    within(session.close()).await;
    assert!(id.is_some());
}
