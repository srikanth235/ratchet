//! P1 of docs/AGENT_HARNESS_PLAN.md: the harness layer's mapping from `SessionSpec` to each CLI's flags and the new events, from the
//! S0 fixtures. Row by row of the matrix in section 4.1:
//!
//! - one-shot headless, resume: the existing adapter tests plus `session_spec_defaults_*` here
//! - text deltas, rate limit, auth expired: the parser tests below
//! - inject MCP, read-only, system prompt add-on: the `*_args` / `*_files` tests
//! - cost: `usage` assertions in the parser tests; built-in model fallback: `claude_fallback_model_*`
//! - long-lived sessions: `tests/live_sessions.rs`

mod common;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ratchet_agents::testing::{FakeProcessSpec, FakeSpawner};
use ratchet_agents::{
    ClaudeCodeAdapter, ClaudeCodeOptions, ClaudeStreamParser, CliAgentOptions, CodexAdapter,
    CodexAppServerParser, CodexOptions, CodexStreamParser, GrokAcpParser, GrokAdapter, GrokOptions,
    GrokStreamParser, TurnSignal,
};
use ratchet_core::ports::{
    Access, AgentAdapter, AgentEvent, AgentEventType, AgentSpec, CancellationToken, Harness,
    McpServer, PortError, SessionSpec,
};
use serde_json::{Value, json};

use common::{collect, env_of, fixture};

fn of_type(events: &[AgentEvent], t: AgentEventType) -> Vec<&AgentEvent> {
    events.iter().filter(|e| e.event_type == t).collect()
}

fn texts(events: &[&AgentEvent]) -> Vec<String> {
    events.iter().filter_map(|e| e.text.clone()).collect()
}

fn ratchet_server() -> McpServer {
    McpServer {
        name: "ratchet".into(),
        command: "/usr/local/bin/ratchetd".into(),
        args: vec!["mcp".into(), "--session".into(), "tok".into()],
        env: [("RATCHET_SOCKET".to_string(), "/tmp/r.sock".to_string())].into(),
    }
}

fn spec(access: Access) -> SessionSpec {
    SessionSpec {
        workdir: "/work/chat-1".into(),
        prompt: "hello".into(),
        access,
        mcp: vec![ratchet_server()],
        system_append: Some("You are Ratchet's assistant.\nAct through Ratchet tools only.".into()),
        ..SessionSpec::default()
    }
}

fn flag<'a>(args: &'a [String], name: &str) -> Option<&'a str> {
    let at = args.iter().position(|a| a == name)?;
    args.get(at + 1).map(String::as_str)
}

fn has(args: &[String], name: &str) -> bool {
    args.iter().any(|a| a == name)
}

fn cli(spawner: &FakeSpawner, env: &[(&str, &str)]) -> CliAgentOptions {
    CliAgentOptions {
        spawner: Some(Arc::new(spawner.clone())),
        env: Some(env_of(env)),
        kill_grace: Some(Duration::from_millis(20)),
        ..CliAgentOptions::default()
    }
}

// ---------------------------------------------------------------------------------------------------- core types

#[test]
fn session_spec_defaults_reproduce_what_agent_spec_callers_always_got() {
    let agent = AgentSpec {
        workdir: "/w".into(),
        prompt: "p".into(),
        model: Some("m".into()),
        env: Some([("A".to_string(), "1".to_string())].into()),
        max_turns: Some(7),
        mcp: Vec::new(),
    };
    let s = SessionSpec::from(&agent);
    assert_eq!(s.access, Access::Edit { network: true });
    assert!(s.mcp.is_empty() && s.system_append.is_none() && s.resume.is_none());
    assert_eq!(s.agent_spec(), agent);
    // the adapters' arguments for the two forms are the same
    let claude = ClaudeCodeAdapter::new(ClaudeCodeOptions::default());
    assert_eq!(
        claude.build_args(&agent, Some("sid")),
        claude.build_session_args(&s, Some("sid"), None)
    );
    let codex = CodexAdapter::new(CodexOptions::default());
    assert_eq!(
        codex.build_args(&agent, None),
        codex.build_session_args(&s, None)
    );
}

#[test]
fn new_event_types_have_stable_wire_names() {
    for (t, name) in [
        (AgentEventType::Delta, "delta"),
        (AgentEventType::Thinking, "thinking"),
        (AgentEventType::RateLimited, "rate_limited"),
        (AgentEventType::AuthExpired, "auth_expired"),
    ] {
        let e = AgentEvent::new(1, t).with_text("x");
        let v = serde_json::to_value(&e).unwrap();
        assert_eq!(v["type"], name);
        let back: AgentEvent = serde_json::from_value(v).unwrap();
        assert_eq!(back, e);
    }
}

// ---------------------------------------------------------------------------------------------------- Claude Code

#[test]
fn claude_read_only_args_drop_every_write_tool_and_never_prompt() {
    let a = ClaudeCodeAdapter::new(ClaudeCodeOptions {
        // a user setting that would widen the session is overruled
        allowed_tools: Some(vec!["Bash".into(), "Edit".into(), "Read".into()]),
        ..ClaudeCodeOptions::default()
    });
    let args = a.build_session_args(
        &spec(Access::ReadOnly),
        None,
        Some(Path::new("/tmp/m.json")),
    );
    assert_eq!(flag(&args, "--permission-mode"), Some("dontAsk"));
    assert_eq!(
        flag(&args, "--allowedTools"),
        Some("Read,Glob,Grep,mcp__ratchet")
    );
    let denied = flag(&args, "--disallowedTools").unwrap();
    for tool in ["Bash", "Edit", "Write", "MultiEdit", "NotebookEdit"] {
        assert!(denied.split(',').any(|t| t == tool), "{tool} denied");
    }
}

#[test]
fn claude_edit_args_keep_the_adapter_options_and_can_close_the_network() {
    let a = ClaudeCodeAdapter::new(ClaudeCodeOptions::default());
    let open = a.build_session_args(&spec(Access::Edit { network: true }), None, None);
    assert_eq!(flag(&open, "--permission-mode"), Some("acceptEdits"));
    assert_eq!(
        flag(&open, "--allowedTools"),
        Some("Bash,Read,Edit,Write,Glob,Grep,mcp__ratchet")
    );
    assert!(!has(&open, "--disallowedTools"));
    let closed = a.build_session_args(&spec(Access::Edit { network: false }), None, None);
    assert_eq!(
        flag(&closed, "--disallowedTools"),
        Some("WebFetch,WebSearch")
    );
}

#[test]
fn claude_mcp_system_append_and_fallback_model_flags() {
    let a = ClaudeCodeAdapter::new(ClaudeCodeOptions {
        append_system_prompt: Some("house rules".into()),
        fallback_model: Some("sonnet".into()),
        ..ClaudeCodeOptions::default()
    });
    let args = a.build_session_args(
        &spec(Access::ReadOnly),
        Some("sid-1"),
        Some(Path::new("/tmp/m.json")),
    );
    assert_eq!(flag(&args, "--mcp-config"), Some("/tmp/m.json"));
    assert!(has(&args, "--strict-mcp-config"));
    assert_eq!(flag(&args, "--fallback-model"), Some("sonnet"));
    assert_eq!(flag(&args, "--resume"), Some("sid-1"));
    assert_eq!(
        flag(&args, "--append-system-prompt"),
        Some("house rules\n\nYou are Ratchet's assistant.\nAct through Ratchet tools only.")
    );
    // no servers, no MCP flags: the user's own configuration stays in force for plain callers
    let plain = a.build_session_args(&SessionSpec::default(), None, None);
    assert!(!has(&plain, "--mcp-config") && !has(&plain, "--strict-mcp-config"));
}

#[test]
fn claude_live_args_stream_json_in_and_out_with_partial_messages() {
    let a = ClaudeCodeAdapter::new(ClaudeCodeOptions::default());
    let args = a.build_live_args(&spec(Access::ReadOnly), None, None);
    assert_eq!(flag(&args, "--input-format"), Some("stream-json"));
    assert_eq!(flag(&args, "--output-format"), Some("stream-json"));
    for f in [
        "-p",
        "--verbose",
        "--include-partial-messages",
        "--replay-user-messages",
    ] {
        assert!(has(&args, f), "{f}");
    }
}

#[tokio::test]
async fn claude_one_shot_writes_the_mcp_config_for_the_run_and_removes_it_after() {
    let seen: Arc<Mutex<Vec<(String, Value)>>> = Arc::default();
    let sink = seen.clone();
    let fake = FakeSpawner::with_plan(move |call, _| {
        let path = flag(&call.args, "--mcp-config").unwrap().to_string();
        let json = serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        sink.lock().unwrap().push((path, json));
        FakeProcessSpec::stdout(fixture("claude-success.jsonl"))
    });
    let a = ClaudeCodeAdapter::new(ClaudeCodeOptions {
        cli: cli(&fake, &[("PATH", "/bin")]),
        allow_subscription_auth: true,
        ..ClaudeCodeOptions::default()
    });
    let session = a
        .start_session(&spec(Access::ReadOnly), &|_| {}, None)
        .await
        .unwrap();
    assert!(!session.id.is_empty());
    let seen = seen.lock().unwrap();
    let (path, config) = &seen[0];
    assert_eq!(
        config["mcpServers"]["ratchet"]["command"],
        "/usr/local/bin/ratchetd"
    );
    assert_eq!(
        config["mcpServers"]["ratchet"]["args"],
        json!(["mcp", "--session", "tok"])
    );
    assert_eq!(
        config["mcpServers"]["ratchet"]["env"]["RATCHET_SOCKET"],
        "/tmp/r.sock"
    );
    assert!(
        !Path::new(path).exists(),
        "the config is removed after the run"
    );
    // the prompt still goes on stdin
    assert_eq!(fake.call(0).stdin, "hello");
}

#[tokio::test]
async fn claude_start_session_with_resume_continues_that_session() {
    let fake = FakeSpawner::new(FakeProcessSpec::stdout(fixture("claude-resume.jsonl")));
    let a = ClaudeCodeAdapter::new(ClaudeCodeOptions {
        cli: cli(&fake, &[("PATH", "/bin")]),
        allow_subscription_auth: true,
        ..ClaudeCodeOptions::default()
    });
    let s = SessionSpec {
        workdir: "/w".into(),
        prompt: "next".into(),
        resume: Some("5f1c1b8e-2d7a-4a37-9c0e-7b6f3b1d0a11".into()),
        ..SessionSpec::default()
    };
    let session = a.start_session(&s, &|_| {}, None).await.unwrap();
    assert_eq!(session.id, "5f1c1b8e-2d7a-4a37-9c0e-7b6f3b1d0a11");
    assert_eq!(
        flag(&fake.call(0).args, "--resume"),
        Some("5f1c1b8e-2d7a-4a37-9c0e-7b6f3b1d0a11")
    );
}

fn claude_events(name: &str, chunk: usize) -> (ClaudeStreamParser, Vec<AgentEvent>) {
    let mut p = ClaudeStreamParser::new(|| 1_000);
    let text = fixture(name);
    let chars: Vec<char> = text.chars().collect();
    let mut events: Vec<AgentEvent> = chars
        .chunks(chunk)
        .flat_map(|c| p.push(&c.iter().collect::<String>()))
        .collect();
    events.extend(p.end());
    (p, events)
}

#[test]
fn claude_deltas_arrive_before_the_whole_message_and_thinking_is_separate() {
    let (_, events) = claude_events("claude-2.1.316-mock-deltas.jsonl", 11);
    let order: Vec<_> = events.iter().map(|e| e.event_type).collect();
    use AgentEventType::*;
    assert_eq!(order, [Status, Delta, Delta, Delta, Message, Usage]);

    let mut p = ClaudeStreamParser::new(|| 1);
    let line = |d: Value| {
        json!({"type": "stream_event", "event": {"type": "content_block_delta", "index": 0, "delta": d}}).to_string()
    };
    let thinking = p.parse_line(&line(json!({"type": "thinking_delta", "thinking": "hmm"})));
    assert_eq!(thinking[0].event_type, Thinking);
    assert_eq!(thinking[0].text.as_deref(), Some("hmm"));
    // other stream events are bookkeeping, not unknown
    assert!(
        p.parse_line(&line(json!({"type": "signature_delta", "signature": "x"})))
            .is_empty()
    );
    assert!(p.unknown_types.is_empty());
}

#[test]
fn claude_rate_limit_is_classified_once_per_turn_with_the_structured_reset_time() {
    let (p, events) = claude_events("claude-2.1.316-mock-rate-limit.jsonl", 64);
    let limited = of_type(&events, AgentEventType::RateLimited);
    assert_eq!(
        limited.len(),
        1,
        "the event, the assistant line and the result say it once"
    );
    assert_eq!(limited[0].data.as_ref().unwrap()["resetsAt"], 1793614200);
    assert!(of_type(&events, AgentEventType::AuthExpired).is_empty());
    assert!(matches!(
        p.failure,
        Some(ratchet_agents::classify::FailureKind::RateLimited {
            resets_at: Some(1793614200)
        })
    ));
}

#[test]
fn claude_assistant_error_and_http_status_classify_without_the_rate_limit_event() {
    let mut p = ClaudeStreamParser::new(|| 5_000);
    let assistant = json!({"type":"assistant","error":"rate_limit","message":{"content":[{"type":"text","text":"slow down"}]}});
    let events = p.parse_line(&assistant.to_string());
    assert_eq!(events[0].event_type, AgentEventType::RateLimited);
    assert!(
        events[0]
            .data
            .as_ref()
            .is_none_or(|d| d.get("resetsAt").is_none())
    );

    let mut p = ClaudeStreamParser::new(|| 5_000);
    let result = json!({"type":"result","subtype":"success","is_error":true,"result":"upstream said no","api_error_status":401});
    let kinds: Vec<_> = p
        .parse_line(&result.to_string())
        .iter()
        .map(|e| e.event_type)
        .collect();
    assert_eq!(
        kinds,
        [
            AgentEventType::Usage,
            AgentEventType::Error,
            AgentEventType::AuthExpired
        ]
    );

    // an `allowed_warning` is not a rejection
    let mut p = ClaudeStreamParser::new(|| 1);
    let warn = json!({"type":"rate_limit_event","rate_limit_info":{"status":"allowed_warning","resetsAt":1}});
    assert!(p.parse_line(&warn.to_string()).is_empty() && p.failure.is_none());
}

#[tokio::test]
async fn claude_one_shot_failure_carries_the_classified_event_and_the_error() {
    for (name, kind) in [
        (
            "claude-2.1.316-mock-rate-limit.jsonl",
            AgentEventType::RateLimited,
        ),
        (
            "claude-2.1.316-mock-auth-expired.jsonl",
            AgentEventType::AuthExpired,
        ),
    ] {
        let fake = FakeSpawner::new(FakeProcessSpec::stdout(fixture(name)));
        let a = ClaudeCodeAdapter::new(ClaudeCodeOptions {
            cli: cli(&fake, &[("PATH", "/bin")]),
            allow_subscription_auth: true,
            ..ClaudeCodeOptions::default()
        });
        let (events, sink) = collect();
        let err = a
            .start(
                &AgentSpec {
                    workdir: "/w".into(),
                    prompt: "p".into(),
                    ..AgentSpec::default()
                },
                &sink,
                None,
            )
            .await
            .unwrap_err();
        assert!(matches!(err, PortError::AgentFailed { .. }), "{name}");
        let events = events.lock().unwrap();
        assert_eq!(of_type(&events, kind).len(), 1, "{name}");
    }
}

// ---------------------------------------------------------------------------------------------------- Codex

#[test]
fn codex_read_only_and_edit_sandbox_values() {
    let a = CodexAdapter::new(CodexOptions {
        // a user option that would widen the session is overruled
        sandbox: Some(ratchet_agents::CodexSandbox::DangerFullAccess),
        ..CodexOptions::default()
    });
    let read_only = a.config_values(&spec(Access::ReadOnly));
    assert_eq!(read_only[0], "sandbox_mode=\"read-only\"");
    assert!(!read_only.iter().any(|v| v.contains("network_access")));

    let a = CodexAdapter::new(CodexOptions::default());
    let edit = a.config_values(&spec(Access::Edit { network: true }));
    assert_eq!(edit[0], "sandbox_mode=\"workspace-write\"");
    assert!(edit.contains(&"sandbox_workspace_write.network_access=true".to_string()));
    let offline = a.config_values(&spec(Access::Edit { network: false }));
    assert!(!offline.iter().any(|v| v.contains("network_access")));
}

#[test]
fn codex_mcp_and_instructions_go_in_dash_c_values_by_default() {
    let a = CodexAdapter::new(CodexOptions::default());
    let values = a.config_values(&spec(Access::ReadOnly));
    for want in [
        "mcp_servers.ratchet.command=\"/usr/local/bin/ratchetd\"",
        "mcp_servers.ratchet.args=[\"mcp\", \"--session\", \"tok\"]",
        "mcp_servers.ratchet.env.RATCHET_SOCKET=\"/tmp/r.sock\"",
        "developer_instructions=\"You are Ratchet's assistant.\\nAct through Ratchet tools only.\"",
    ] {
        assert!(values.contains(&want.to_string()), "{want}: {values:?}");
    }
    let exec = a.build_session_args(&spec(Access::ReadOnly), Some("thread-1"));
    assert_eq!(exec[..3], ["exec", "resume", "thread-1"]);
    assert!(
        exec.windows(2)
            .any(|w| w[0] == "-c" && w[1].starts_with("mcp_servers.ratchet.command="))
    );
    // the live form puts the CLI's own `-c` before the subcommand
    let live = a.build_app_server_args(&spec(Access::ReadOnly));
    assert_eq!(live.last().map(String::as_str), Some("app-server"));
    assert_eq!(live[0], "-c");
}

#[test]
fn codex_server_names_that_are_not_bare_keys_are_quoted() {
    let mut s = spec(Access::ReadOnly);
    s.mcp[0].name = "my server".into();
    let values = CodexAdapter::new(CodexOptions::default()).config_values(&s);
    assert!(
        values
            .iter()
            .any(|v| v.starts_with("mcp_servers.\"my server\".command="))
    );
}

#[tokio::test]
async fn codex_overlay_home_replaces_the_flags_and_links_the_login() {
    let source = tempfile::tempdir().unwrap();
    std::fs::write(source.path().join("auth.json"), "{\"token\":1}").unwrap();
    std::fs::write(
        source.path().join("config.toml"),
        "[mcp_servers.theirs]\ncommand=\"x\"\n",
    )
    .unwrap();
    // (home, config.toml, AGENTS.md, auth.json, any -c mcp flag)
    type Overlay = (String, String, String, String, bool);
    let seen: Arc<Mutex<Vec<Overlay>>> = Arc::default();
    let sink = seen.clone();
    let fake = FakeSpawner::with_plan(move |call, _| {
        let home = call.options.env["CODEX_HOME"].clone();
        let read = |n: &str| std::fs::read_to_string(Path::new(&home).join(n)).unwrap_or_default();
        sink.lock().unwrap().push((
            home.clone(),
            read("config.toml"),
            read("AGENTS.md"),
            read("auth.json"),
            call.args.iter().any(|a| a.contains("mcp_servers")),
        ));
        FakeProcessSpec::stdout(fixture("codex-success.jsonl"))
    });
    let a = CodexAdapter::new(CodexOptions {
        cli: cli(&fake, &[("PATH", "/bin")]),
        config_overlay: true,
        codex_home: Some(source.path().to_string_lossy().into_owned()),
        allow_subscription_auth: true,
        ..CodexOptions::default()
    });
    a.start_session(&spec(Access::ReadOnly), &|_| {}, None)
        .await
        .unwrap();
    let seen = seen.lock().unwrap();
    let (home, config, agents, auth, flags) = &seen[0];
    assert!(
        config.contains("[mcp_servers.ratchet]")
            && config.contains("command = \"/usr/local/bin/ratchetd\"")
    );
    assert!(config.contains("[mcp_servers.ratchet.env]"));
    assert!(
        !config.contains("theirs"),
        "the user's own servers are not loaded"
    );
    assert!(agents.starts_with("You are Ratchet's assistant."));
    assert_eq!(auth, "{\"token\":1}", "the login is linked in");
    assert!(!flags, "no -c mcp flags in overlay mode");
    assert!(
        !Path::new(home).exists(),
        "the overlay is removed after the run"
    );
}

#[tokio::test]
async fn codex_one_shot_passes_both_api_key_variables_and_the_prompt_on_stdin() {
    let fake = FakeSpawner::new(FakeProcessSpec::stdout(fixture("codex-success.jsonl")));
    let a = CodexAdapter::new(CodexOptions {
        cli: cli(&fake, &[("OPENAI_API_KEY", "sk"), ("PATH", "/bin")]),
        ..CodexOptions::default()
    });
    a.start_session(&spec(Access::ReadOnly), &|_| {}, None)
        .await
        .unwrap();
    let call = fake.call(0);
    assert_eq!(call.options.env["CODEX_API_KEY"], "sk");
    assert_eq!(call.stdin, "hello");
    assert_eq!(call.args.last().map(String::as_str), Some("-"));
}

fn codex_app(name: &str) -> (CodexAppServerParser, Vec<AgentEvent>) {
    let mut p = CodexAppServerParser::new(|| 2_000, None);
    let mut events = Vec::new();
    for line in fixture(name).lines().filter(|l| !l.trim().is_empty()) {
        events.extend(p.parse_line(line));
    }
    (p, events)
}

#[test]
fn codex_app_server_deltas_come_in_order_then_the_whole_message_and_the_turn_usage() {
    let (p, events) = codex_app("codex-0.166.0-mock-deltas.jsonl");
    let order: Vec<_> = events.iter().map(|e| e.event_type).collect();
    use AgentEventType::*;
    assert_eq!(order, [Status, Delta, Delta, Delta, Message, Usage]);
    assert_eq!(
        texts(&of_type(&events, Delta)),
        ["Three ", "optimizations ", "are running."]
    );
    assert_eq!(
        p.thread_id.as_deref(),
        Some("0199b3a0-5c1e-7a40-9d2e-6f1a2b3c4d5e")
    );
    assert_eq!(p.last_signal, Some(TurnSignal::Completed));
    let usage = of_type(&events, Usage)[0].usage.unwrap();
    assert_eq!((usage.input_tokens, usage.output_tokens), (5200, 240));
    assert!(p.unknown_types.is_empty() && p.malformed_lines == 0);
}

#[test]
fn codex_app_server_usage_is_the_growth_of_the_thread_total_and_priced_from_the_table() {
    let mut p = CodexAppServerParser::new(
        || 1,
        Some(ratchet_agents::CodexPricing {
            input_per_m: 2.0,
            cached_input_per_m: Some(0.5),
            output_per_m: 10.0,
        }),
    );
    let total = |i: u64, c: u64, o: u64| {
        json!({"method":"thread/tokenUsage/updated","params":{"tokenUsage":{"total":{"inputTokens":i,"cachedInputTokens":c,"outputTokens":o}}}}).to_string()
    };
    let done = json!({"method":"turn/completed","params":{"turn":{"id":"t","status":"completed"}}})
        .to_string();
    p.parse_line(&total(1000, 600, 100));
    let first = p.parse_line(&done);
    let u = first[0].usage.unwrap();
    assert_eq!((u.input_tokens, u.output_tokens), (1000, 100));
    // (400 uncached * 2 + 600 cached * 0.5 + 100 * 10) / 1e6
    assert!((u.cost_usd.unwrap() - 0.0021).abs() < 1e-9);
    p.parse_line(&total(1500, 900, 160));
    let u = p.parse_line(&done)[0].usage.unwrap();
    assert_eq!((u.input_tokens, u.output_tokens), (500, 60));
}

#[test]
fn codex_app_server_two_turn_session_reports_each_turn() {
    let (p, events) = codex_app("codex-0.166.0-mock-session.jsonl");
    assert_eq!(
        texts(&of_type(&events, AgentEventType::Message)),
        [
            "Hello. Ask me about your optimizations.",
            "The newest one is paused."
        ]
    );
    assert_eq!(of_type(&events, AgentEventType::Usage).len(), 2);
    assert_eq!(p.turn_id.as_deref(), Some("turn_2"));
}

#[test]
fn codex_app_server_rate_limit_carries_the_reset_time_from_the_account_notification() {
    let (p, events) = codex_app("codex-0.166.0-mock-app-server-rate-limit.jsonl");
    let limited = of_type(&events, AgentEventType::RateLimited);
    assert_eq!(limited.len(), 1);
    assert_eq!(limited[0].data.as_ref().unwrap()["resetsAt"], 1793614200);
    assert!(limited[0].text.as_deref().unwrap().contains("usage limit"));
    assert!(matches!(p.last_signal, Some(TurnSignal::Failed(ref m)) if m.contains("usage limit")));
    assert_eq!(
        of_type(&events, AgentEventType::Error).len(),
        1,
        "reported once"
    );
}

#[test]
fn codex_app_server_unauthorized_is_auth_expired() {
    let (p, events) = codex_app("codex-0.166.0-mock-app-server-auth-expired.jsonl");
    assert_eq!(of_type(&events, AgentEventType::AuthExpired).len(), 1);
    assert!(of_type(&events, AgentEventType::RateLimited).is_empty());
    assert!(matches!(p.last_signal, Some(TurnSignal::Failed(_))));
}

#[test]
fn codex_app_server_mcp_tool_call_and_refused_command_are_tool_events() {
    let (p, events) = codex_app("codex-0.166.0-mock-app-server-mcp-tool.jsonl");
    let tools = of_type(&events, AgentEventType::Tool);
    let phases: Vec<_> = tools
        .iter()
        .map(|e| e.data.as_ref().unwrap()["phase"].as_str().unwrap())
        .collect();
    assert_eq!(phases, ["call", "result", "call", "result"]);
    assert_eq!(tools[0].text.as_deref(), Some("ratchet.list_optimizations"));
    assert_eq!(tools[1].data.as_ref().unwrap()["isError"], false);
    assert!(tools[1].text.as_deref().unwrap().contains("opt_1"));
    assert_eq!(tools[3].data.as_ref().unwrap()["isError"], true);
    assert!(
        tools[3]
            .text
            .as_deref()
            .unwrap()
            .contains("Read-only file system")
    );
    assert!(p.unknown_types.is_empty());
}

#[test]
fn codex_app_server_interrupted_turn_is_a_status_not_an_error() {
    let (p, events) = codex_app("codex-0.166.0-mock-app-server-interrupt.jsonl");
    assert!(of_type(&events, AgentEventType::Error).is_empty());
    assert!(
        events
            .iter()
            .any(|e| e.text.as_deref() == Some("codex turn interrupted"))
    );
    assert_eq!(
        p.last_signal,
        Some(TurnSignal::Completed),
        "the second turn finished"
    );
}

#[test]
fn codex_exec_reasoning_is_thinking_only_when_asked_and_failures_are_classified() {
    let text = fixture("codex-success.jsonl");
    let run = |thinking: bool| {
        let mut p = CodexStreamParser::new(|| 1, None);
        p.emit_thinking = thinking;
        let mut events = p.push(&text);
        events.extend(p.end());
        events
    };
    assert!(of_type(&run(false), AgentEventType::Thinking).is_empty());
    assert_eq!(
        texts(&of_type(&run(true), AgentEventType::Thinking)),
        ["**Planning the profile**"]
    );

    let classify = |name: &str| {
        let mut p = CodexStreamParser::new(|| 3_000, None);
        let mut events = p.push(&fixture(name));
        events.extend(p.end());
        events
    };
    // `exec` has no structured code: the text decides, and the reset time is only in the prose
    let limited = classify("codex-0.166.0-mock-rate-limit.jsonl");
    assert_eq!(of_type(&limited, AgentEventType::RateLimited).len(), 1);
    assert_eq!(of_type(&limited, AgentEventType::Error).len(), 2);
    let auth = classify("codex-0.166.0-mock-auth-expired.jsonl");
    assert_eq!(of_type(&auth, AgentEventType::AuthExpired).len(), 1);
    assert!(
        of_type(&auth, AgentEventType::RateLimited).is_empty(),
        "'Reconnecting... 1/5' is neither"
    );
}

// ---------------------------------------------------------------------------------------------------- Grok

#[test]
fn grok_read_only_args_use_the_sandbox_and_the_tool_lists() {
    let a = GrokAdapter::new(GrokOptions::default());
    let args = a.build_session_args(&spec(Access::ReadOnly), None, Path::new("/tmp/p.txt"));
    assert_eq!(flag(&args, "--prompt-file"), Some("/tmp/p.txt"));
    assert!(!has(&args, "-p"));
    assert_eq!(flag(&args, "--sandbox"), Some("read-only"));
    assert_eq!(flag(&args, "--tools"), Some("read_file,list_dir,grep,glob"));
    assert!(
        flag(&args, "--disallowed-tools")
            .unwrap()
            .contains("run_terminal_command")
    );
    assert!(
        !has(&args, "--permission-mode"),
        "nobody can approve, so nothing asks"
    );
    assert_eq!(
        flag(&args, "--rules"),
        Some("You are Ratchet's assistant.\nAct through Ratchet tools only.")
    );
    let edit = a.build_session_args(
        &spec(Access::Edit { network: true }),
        Some("sid"),
        Path::new("/tmp/p.txt"),
    );
    assert_eq!(flag(&edit, "--permission-mode"), Some("acceptEdits"));
    assert_eq!(flag(&edit, "--resume"), Some("sid"));
    assert!(!has(&edit, "--tools") && !has(&edit, "--sandbox"));
}

#[tokio::test]
async fn grok_overlay_home_holds_the_session_servers_and_the_api_key_goes_under_both_names() {
    let source = tempfile::tempdir().unwrap();
    std::fs::write(source.path().join("auth.json"), "{\"cached\":1}").unwrap();
    let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
    let sink = seen.clone();
    let fake = FakeSpawner::with_plan(move |call, _| {
        let home = call.options.env["GROK_HOME"].clone();
        let read = |n: &str| std::fs::read_to_string(Path::new(&home).join(n)).unwrap_or_default();
        sink.lock()
            .unwrap()
            .push((read("config.toml"), read("auth.json")));
        FakeProcessSpec::stdout(fixture("grok-success.jsonl"))
    });
    // only the second name is set: both reach the child
    let a = GrokAdapter::new(GrokOptions {
        cli: cli(
            &fake,
            &[("GROK_CODE_XAI_API_KEY", "xai-2"), ("PATH", "/bin")],
        ),
        grok_home: Some(source.path().to_string_lossy().into_owned()),
        ..GrokOptions::default()
    });
    a.start_session(&spec(Access::ReadOnly), &|_| {}, None)
        .await
        .unwrap();
    let call = fake.call(0);
    assert_eq!(call.options.env["XAI_API_KEY"], "xai-2");
    assert_eq!(call.options.env["GROK_CODE_XAI_API_KEY"], "xai-2");
    let seen = seen.lock().unwrap();
    let (config, auth) = &seen[0];
    assert!(
        config.contains("[mcp_servers.ratchet]")
            && config.contains("args = [\"mcp\", \"--session\", \"tok\"]")
    );
    assert!(config.contains("[compat.claude]\nmcps = false"));
    assert_eq!(auth, "{\"cached\":1}");
    // no servers: no overlay at all
    let scratch = ratchet_agents::Scratch::new("test").unwrap();
    assert_eq!(
        a.write_overlay(&SessionSpec::default(), &scratch).unwrap(),
        None
    );
}

fn grok_exec(name: &str, deltas: bool) -> (GrokStreamParser, Vec<AgentEvent>) {
    let mut p = GrokStreamParser::new(|| 4_000);
    p.emit_deltas = deltas;
    let mut events = p.push(&fixture(name));
    events.extend(p.end());
    (p, events)
}

#[test]
fn grok_exec_deltas_are_off_for_optimization_runs_and_on_for_chat() {
    let (_, quiet) = grok_exec("grok-1.0.70-mock-deltas.jsonl", false);
    assert!(of_type(&quiet, AgentEventType::Delta).is_empty());
    let (_, chat) = grok_exec("grok-1.0.70-mock-deltas.jsonl", true);
    assert_eq!(
        texts(&of_type(&chat, AgentEventType::Delta)),
        ["Three ", "optimizations ", "are running."]
    );
    assert_eq!(
        texts(&of_type(&chat, AgentEventType::Thinking)),
        ["Counting runs."]
    );
    // the joined message and the dollars are unchanged
    assert_eq!(
        texts(&of_type(&chat, AgentEventType::Message)),
        ["Three optimizations are running."]
    );
    assert_eq!(
        of_type(&chat, AgentEventType::Usage)[0]
            .usage
            .unwrap()
            .cost_usd,
        Some(0.004)
    );
}

#[test]
fn grok_exec_failures_are_classified_by_their_text() {
    let (_, limited) = grok_exec("grok-1.0.70-mock-rate-limit.jsonl", false);
    let l = of_type(&limited, AgentEventType::RateLimited);
    assert_eq!(l.len(), 1);
    // "retry after 30s" relative to the parser's clock (4 s since epoch)
    assert_eq!(l[0].data.as_ref().unwrap()["resetsAt"], 34);
    let (_, auth) = grok_exec("grok-1.0.70-mock-auth-expired.jsonl", false);
    assert_eq!(of_type(&auth, AgentEventType::AuthExpired).len(), 1);
}

fn grok_acp(name: &str) -> (GrokAcpParser, Vec<AgentEvent>) {
    let mut p = GrokAcpParser::new(|| 6_000);
    let mut events = Vec::new();
    for line in fixture(name).lines().filter(|l| !l.trim().is_empty()) {
        events.extend(p.parse_line(line));
    }
    (p, events)
}

#[test]
fn grok_acp_chunks_are_deltas_and_the_prompt_response_closes_the_message() {
    let (p, events) = grok_acp("grok-1.0.70-mock-session.jsonl");
    use AgentEventType::*;
    assert_eq!(
        events.iter().map(|e| e.event_type).collect::<Vec<_>>(),
        [Delta, Delta, Message, Delta, Delta, Message]
    );
    assert_eq!(
        texts(&of_type(&events, Message)),
        [
            "Hello. Ask me about your optimizations.",
            "The newest one is paused."
        ]
    );
    assert_eq!(
        p.session_id.as_deref(),
        Some("01a2b3c4-d5e6-7f80-9a1b-2c3d4e5f6a7b")
    );
    assert!(p.unknown_types.is_empty() && p.malformed_lines == 0);
}

#[test]
fn grok_acp_tool_calls_thoughts_usage_and_cancel() {
    let (_, events) = grok_acp("grok-1.0.70-mock-acp-mcp-tool.jsonl");
    let tools = of_type(&events, AgentEventType::Tool);
    assert_eq!(tools.len(), 4);
    assert_eq!(
        tools[0].data.as_ref().unwrap()["name"],
        "mcp__ratchet__list_optimizations"
    );
    assert_eq!(tools[1].data.as_ref().unwrap()["isError"], false);
    // the refused write is an errored tool result, not a failed turn
    assert_eq!(tools[3].data.as_ref().unwrap()["isError"], true);
    assert!(of_type(&events, AgentEventType::Error).is_empty());
    let usage = of_type(&events, AgentEventType::Usage)[0].usage.unwrap();
    assert_eq!(
        (usage.input_tokens, usage.output_tokens, usage.cost_usd),
        (812, 45, Some(0.004))
    );

    let (_, events) = grok_acp("grok-1.0.70-mock-acp-interrupt.jsonl");
    assert_eq!(
        texts(&of_type(&events, AgentEventType::Thinking)),
        ["Counting."]
    );
    assert!(
        of_type(&events, AgentEventType::Error).is_empty(),
        "cancelled is a normal end"
    );
}

#[test]
fn grok_acp_errors_are_classified() {
    let (_, limited) = grok_acp("grok-1.0.70-mock-acp-rate-limit.jsonl");
    let l = of_type(&limited, AgentEventType::RateLimited);
    assert_eq!(l.len(), 1);
    assert_eq!(l[0].data.as_ref().unwrap()["resetsAt"], 36);
    let (_, auth) = grok_acp("grok-1.0.70-mock-acp-auth-expired.jsonl");
    assert_eq!(of_type(&auth, AgentEventType::AuthExpired).len(), 1);
    assert_eq!(of_type(&auth, AgentEventType::Error).len(), 1);
}

#[tokio::test]
async fn grok_one_shot_failure_returns_agent_failed_with_the_classified_event() {
    for (name, kind) in [
        (
            "grok-1.0.70-mock-rate-limit.jsonl",
            AgentEventType::RateLimited,
        ),
        (
            "grok-1.0.70-mock-auth-expired.jsonl",
            AgentEventType::AuthExpired,
        ),
    ] {
        let fake = FakeSpawner::new(FakeProcessSpec::stdout(fixture(name)).with_exit(1));
        let a = GrokAdapter::new(GrokOptions {
            cli: cli(&fake, &[("XAI_API_KEY", "k"), ("PATH", "/bin")]),
            ..GrokOptions::default()
        });
        let (events, sink) = collect();
        let err = a
            .start(
                &AgentSpec {
                    workdir: "/w".into(),
                    prompt: "p".into(),
                    ..AgentSpec::default()
                },
                &sink,
                Some(&CancellationToken::new()),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, PortError::AgentFailed { .. }), "{name}");
        assert_eq!(of_type(&events.lock().unwrap(), kind).len(), 1, "{name}");
    }
}

// ---------------------------------------------------------------------------------------------------- capabilities

#[test]
fn capabilities_say_what_each_cli_can_do_and_the_live_flag_follows_the_option() {
    let claude = ClaudeCodeAdapter::new(ClaudeCodeOptions::default()).capabilities();
    let codex = CodexAdapter::new(CodexOptions::default()).capabilities();
    let grok = GrokAdapter::new(GrokOptions::default()).capabilities();
    assert!(
        claude.live_session
            && claude.streaming
            && claude.mcp
            && claude.read_only
            && claude.cost_in_usd
    );
    assert!(codex.live_session && codex.streaming && codex.mcp && codex.read_only);
    assert!(!codex.cost_in_usd, "Codex reports tokens, not dollars");
    assert!(grok.live_session && grok.cost_in_usd);

    let off = CliAgentOptions {
        disable_live_session: true,
        ..CliAgentOptions::default()
    };
    let c = CodexAdapter::new(CodexOptions {
        cli: off,
        ..CodexOptions::default()
    })
    .capabilities();
    assert!(
        !c.live_session && !c.streaming,
        "no app-server, no deltas: whole messages only"
    );
}
