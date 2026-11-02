//! S0 of docs/AGENT_HARNESS_PLAN.md: the mock fixtures (`<cli>-<version>-mock-<behaviour>.jsonl`, plus one real Claude recording)
//! are tolerated by the current parsers, and the fake CLI (`tests/fake_cli`) replays them and speaks the long-lived protocols.
//!
//! Nothing here changes parser behaviour. Where a fixture shows something the parsers do not understand yet (Codex app-server,
//! Grok ACP, stream deltas, rate-limit classification) the test pins what happens today: the line is counted, never fatal.

mod common;

use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{Receiver, channel};
use std::time::Duration;

use ratchet_agents::{ClaudeStreamParser, CodexStreamParser, GrokStreamParser};
use ratchet_core::ports::{AgentEvent, AgentEventType};
use serde_json::{Value, json};

use common::fixture;

fn lines(name: &str) -> usize {
    fixture(name)
        .lines()
        .filter(|l| !l.trim().is_empty())
        .count()
}

fn of_type(events: &[AgentEvent], t: AgentEventType) -> Vec<&AgentEvent> {
    events.iter().filter(|e| e.event_type == t).collect()
}

/// (events, malformed_lines, unknown_types) from one parser run, fed whole and then in 7-char chunks; both must agree.
struct Parsed {
    events: Vec<AgentEvent>,
    malformed: usize,
    unknown: Vec<String>,
}

fn chunked(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    chars.chunks(7).map(|c| c.iter().collect()).collect()
}

fn claude(name: &str) -> (ClaudeStreamParser, Parsed) {
    let run = |chunks: Vec<String>| {
        let mut p = ClaudeStreamParser::new(|| 1);
        let mut events: Vec<AgentEvent> = chunks.iter().flat_map(|c| p.push(c)).collect();
        events.extend(p.end());
        (p, events)
    };
    let text = fixture(name);
    let (p, events) = run(vec![text.clone()]);
    let (p2, events2) = run(chunked(&text));
    assert_eq!(
        events.len(),
        events2.len(),
        "{name}: chunking changes events"
    );
    assert_eq!(p.unknown_types, p2.unknown_types, "{name}: chunking");
    let parsed = Parsed {
        events,
        malformed: p.malformed_lines,
        unknown: p.unknown_types.iter().cloned().collect(),
    };
    (p, parsed)
}

fn codex(name: &str) -> (CodexStreamParser, Parsed) {
    let run = |chunks: Vec<String>| {
        let mut p = CodexStreamParser::new(|| 1, None);
        let mut events: Vec<AgentEvent> = chunks.iter().flat_map(|c| p.push(c)).collect();
        events.extend(p.end());
        (p, events)
    };
    let text = fixture(name);
    let (p, events) = run(vec![text.clone()]);
    let (p2, events2) = run(chunked(&text));
    assert_eq!(
        events.len(),
        events2.len(),
        "{name}: chunking changes events"
    );
    assert_eq!(p.malformed_lines, p2.malformed_lines, "{name}: chunking");
    let parsed = Parsed {
        events,
        malformed: p.malformed_lines,
        unknown: p.unknown_types.iter().cloned().collect(),
    };
    (p, parsed)
}

fn grok(name: &str) -> (GrokStreamParser, Parsed) {
    let run = |chunks: Vec<String>| {
        let mut p = GrokStreamParser::new(|| 1);
        let mut events: Vec<AgentEvent> = chunks.iter().flat_map(|c| p.push(c)).collect();
        events.extend(p.end());
        (p, events)
    };
    let text = fixture(name);
    let (p, events) = run(vec![text.clone()]);
    let (p2, events2) = run(chunked(&text));
    assert_eq!(
        events.len(),
        events2.len(),
        "{name}: chunking changes events"
    );
    assert_eq!(p.malformed_lines, p2.malformed_lines, "{name}: chunking");
    let parsed = Parsed {
        events,
        malformed: p.malformed_lines,
        unknown: p.unknown_types.iter().cloned().collect(),
    };
    (p, parsed)
}

fn texts(events: &[&AgentEvent]) -> Vec<String> {
    events.iter().filter_map(|e| e.text.clone()).collect()
}

// ------------------------------------------------------------------------------------------------------- Claude Code

const CLAUDE_SID: &str = "9c1e5d7a-0b2f-4c63-8a41-5d2f7e6b3c10";

#[test]
fn claude_real_recording_with_partial_messages_counts_stream_and_rate_limit_events_as_unknown() {
    let (p, r) = claude("claude-2.1.316-real-deltas.jsonl");
    assert_eq!(p.session_id.as_deref(), Some(CLAUDE_SID));
    assert_eq!(r.malformed, 0);
    // Deltas are not reported yet (P1): they are counted, never fatal. The whole message still arrives.
    // `system/status` (a real subtype) is also unknown today.
    assert_eq!(
        r.unknown,
        ["rate_limit_event", "stream_event", "system:status"]
    );
    let messages = of_type(&r.events, AgentEventType::Message);
    assert_eq!(texts(&messages), ["Hi! What would you like to work on?"]);
    assert!(p.result.as_ref().is_some_and(|x| !x.is_error));
}

#[test]
fn claude_mock_deltas_and_session_parse_like_the_real_shape() {
    let (p, r) = claude("claude-2.1.316-mock-deltas.jsonl");
    assert_eq!(r.unknown, ["stream_event"]);
    assert_eq!(r.malformed, 0);
    assert_eq!(
        texts(&of_type(&r.events, AgentEventType::Message)),
        ["Three optimizations are running."]
    );
    assert_eq!(p.session_id.as_deref(), Some(CLAUDE_SID));

    // Two turns in one process: one Usage event per `result`, the second `result` wins.
    let (p, r) = claude("claude-2.1.316-mock-session.jsonl");
    assert_eq!(of_type(&r.events, AgentEventType::Usage).len(), 2);
    assert_eq!(texts(&of_type(&r.events, AgentEventType::Message)).len(), 2);
    assert_eq!(p.result.as_ref().map(|x| x.num_turns), Some(2));
    assert_eq!(r.unknown, ["stream_event"]);
}

#[test]
fn claude_mock_mcp_tool_call_is_a_tool_call_and_a_tool_result() {
    let (p, r) = claude("claude-2.1.316-mock-mcp-tool.jsonl");
    assert!(r.unknown == ["stream_event"] && r.malformed == 0);
    let tools = of_type(&r.events, AgentEventType::Tool);
    assert_eq!(tools.len(), 2);
    let call = tools[0].data.as_ref().unwrap();
    assert_eq!(call["phase"], "call");
    assert_eq!(call["name"], "mcp__ratchet__list_optimizations");
    assert_eq!(tools[1].data.as_ref().unwrap()["isError"], false);
    assert!(p.result.is_some());
}

#[test]
fn claude_mock_read_only_refusal_is_an_errored_tool_result_not_a_failed_turn() {
    let (p, r) = claude("claude-2.1.316-mock-read-only-refusal.jsonl");
    let tools = of_type(&r.events, AgentEventType::Tool);
    assert_eq!(tools[1].data.as_ref().unwrap()["isError"], true);
    assert!(
        tools[1]
            .text
            .as_deref()
            .unwrap()
            .contains("haven't granted it")
    );
    // The turn itself succeeded; the refusal is only visible through the tool result (and `permission_denials`).
    assert!(p.result.as_ref().is_some_and(|x| !x.is_error));
    assert!(of_type(&r.events, AgentEventType::Error).is_empty());
}

#[test]
fn claude_mock_rate_limit_and_auth_expired_end_as_error_results_today() {
    let (p, r) = claude("claude-2.1.316-mock-rate-limit.jsonl");
    // `rate_limit_event` is unknown today; `system/api_retry` is a Status event; the failure is an Error with the CLI's text.
    assert_eq!(r.unknown, ["rate_limit_event"]);
    assert!(p.result.as_ref().is_some_and(|x| x.is_error));
    let errors = of_type(&r.events, AgentEventType::Error);
    assert_eq!(errors.len(), 1);
    assert!(
        errors[0]
            .text
            .as_deref()
            .unwrap()
            .contains("hit your limit")
    );
    assert!(
        r.events
            .iter()
            .any(|e| e.text.as_deref() == Some("claude-code api_retry"))
    );

    let (p, r) = claude("claude-2.1.316-mock-auth-expired.jsonl");
    assert!(r.unknown.is_empty());
    assert!(p.result.as_ref().is_some_and(|x| x.is_error));
    let errors = of_type(&r.events, AgentEventType::Error);
    assert!(errors[0].text.as_deref().unwrap().contains("/login"));
}

// ------------------------------------------------------------------------------------------------------- Codex

#[test]
fn codex_app_server_fixtures_are_counted_as_malformed_lines_never_fatal() {
    // `codex app-server` speaks JSON-RPC (`method`, no `type`); the exec parser has no use for it until P1 adds one.
    for name in [
        "codex-0.166.0-mock-deltas.jsonl",
        "codex-0.166.0-mock-session.jsonl",
    ] {
        let (p, r) = codex(name);
        assert!(r.events.is_empty(), "{name}");
        assert_eq!(r.malformed, lines(name), "{name}");
        assert!(r.unknown.is_empty() && p.thread_id.is_none(), "{name}");
    }
}

#[test]
fn codex_mock_mcp_tool_call_and_read_only_refusal() {
    let (p, r) = codex("codex-0.166.0-mock-mcp-tool.jsonl");
    assert!(r.unknown.is_empty() && r.malformed == 0);
    assert!(p.turn.completed);
    let tools = of_type(&r.events, AgentEventType::Tool);
    assert_eq!(tools[0].text.as_deref(), Some("ratchet.list_optimizations"));

    let (p, r) = codex("codex-0.166.0-mock-read-only-refusal.jsonl");
    assert!(r.unknown.is_empty() && r.malformed == 0);
    // A refused command is an errored result; a refused `file_change` is still reported as a call (status is not read today).
    assert!(
        of_type(&r.events, AgentEventType::Tool)
            .iter()
            .any(|e| e.data.as_ref().is_some_and(|d| d["isError"] == true))
    );
    assert!(p.turn.completed && !p.turn.failed);
}

#[test]
fn codex_mock_rate_limit_and_auth_expired_fail_the_turn_with_the_cli_text() {
    let (p, r) = codex("codex-0.166.0-mock-rate-limit.jsonl");
    assert!(p.turn.failed && !p.turn.completed);
    assert!(p.turn.error.as_deref().unwrap().contains("usage limit"));
    assert!(
        p.thread_id.is_none(),
        "the limit hits before thread.started"
    );
    assert_eq!(of_type(&r.events, AgentEventType::Error).len(), 2);

    let (p, r) = codex("codex-0.166.0-mock-auth-expired.jsonl");
    assert!(p.turn.failed);
    assert!(p.turn.error.as_deref().unwrap().contains("401"));
    assert!(r.unknown.is_empty() && r.malformed == 0);
}

// ------------------------------------------------------------------------------------------------------- Grok

#[test]
fn grok_mock_deltas_join_into_one_message_with_usage() {
    let (p, r) = grok("grok-1.0.70-mock-deltas.jsonl");
    assert!(r.unknown.is_empty() && r.malformed == 0);
    assert_eq!(
        texts(&of_type(&r.events, AgentEventType::Message)),
        ["Three optimizations are running."]
    );
    assert!(p.turn.completed);
    assert_eq!(
        p.session_id.as_deref(),
        Some("01a2b3c4-d5e6-7f80-9a1b-2c3d4e5f6a7b")
    );
}

#[test]
fn grok_acp_session_fixture_is_counted_as_malformed_lines_never_fatal() {
    let name = "grok-1.0.70-mock-session.jsonl";
    let (p, r) = grok(name);
    assert!(r.events.is_empty());
    assert_eq!(r.malformed, lines(name));
    assert!(r.unknown.is_empty() && !p.turn.completed);
}

#[test]
fn grok_mock_mcp_tool_read_only_refusal_rate_limit_and_auth_expired() {
    let (p, r) = grok("grok-1.0.70-mock-mcp-tool.jsonl");
    assert!(p.turn.completed);
    let tools = of_type(&r.events, AgentEventType::Tool);
    assert_eq!(
        tools[0].data.as_ref().unwrap()["name"],
        "mcp__ratchet__list_optimizations"
    );

    let (p, r) = grok("grok-1.0.70-mock-read-only-refusal.jsonl");
    assert!(p.turn.completed);
    assert_eq!(
        of_type(&r.events, AgentEventType::Tool)[1]
            .data
            .as_ref()
            .unwrap()["isError"],
        true
    );

    for (name, needle) in [
        ("grok-1.0.70-mock-rate-limit.jsonl", "rate limit"),
        ("grok-1.0.70-mock-auth-expired.jsonl", "grok login"),
    ] {
        let (p, r) = grok(name);
        assert!(p.turn.failed && !p.turn.completed, "{name}");
        assert!(p.turn.error.as_deref().unwrap().contains(needle), "{name}");
        assert!(r.unknown.is_empty() && r.malformed == 0, "{name}");
    }
}

// ------------------------------------------------------------------------------------------------------- fake CLI

fn fixture_path(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

fn fake_cli(mode: &str, name: &str, extra: &[&str]) -> Child {
    Command::new(env!("CARGO_BIN_EXE_ratchet-fake-cli"))
        .arg(mode)
        .arg(fixture_path(name))
        .args(extra)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("fake cli starts")
}

/// stdout lines on a channel, so a misbehaving fake fails the test by timeout instead of hanging it.
fn stdout_lines(child: &mut Child) -> Receiver<String> {
    let (tx, rx) = channel();
    let out = child.stdout.take().unwrap();
    std::thread::spawn(move || {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });
    rx
}

fn send(child: &mut Child, message: Value) {
    let stdin = child.stdin.as_mut().unwrap();
    writeln!(stdin, "{message}").unwrap();
    stdin.flush().unwrap();
}

/// Read lines until `stop` accepts one (inclusive).
fn read_until(rx: &Receiver<String>, stop: impl Fn(&Value) -> bool) -> Vec<Value> {
    let mut got = Vec::new();
    loop {
        let line = rx
            .recv_timeout(Duration::from_secs(10))
            .expect("fake cli answered in time");
        let value: Value = serde_json::from_str(&line).expect("fake cli prints JSON lines");
        let done = stop(&value);
        got.push(value);
        if done {
            return got;
        }
    }
}

fn silent(rx: &Receiver<String>) -> bool {
    rx.recv_timeout(Duration::from_millis(150)).is_err()
}

fn finish(mut child: Child) {
    drop(child.stdin.take());
    assert!(child.wait().unwrap().success());
}

#[test]
fn fake_cli_replays_a_fixture_with_exit_code_and_stderr() {
    let child = fake_cli(
        "replay",
        "codex-0.166.0-mock-rate-limit.jsonl",
        &["--exit-code", "1", "--stderr", "boom"],
    );
    let out = child.wait_with_output().unwrap();
    assert_eq!(out.status.code(), Some(1));
    assert_eq!(String::from_utf8_lossy(&out.stderr).trim(), "boom");
    // What it prints is exactly what the parser eats.
    let mut p = CodexStreamParser::new(|| 1, None);
    p.push(&String::from_utf8_lossy(&out.stdout));
    p.end();
    assert!(p.turn.failed);
}

#[test]
fn fake_claude_stream_json_session_answers_one_turn_per_user_message() {
    let log = tempfile::NamedTempFile::new().unwrap();
    let mut child = Command::new(env!("CARGO_BIN_EXE_ratchet-fake-cli"))
        .args(["claude-stream-json"])
        .arg(fixture_path("claude-2.1.316-mock-session.jsonl"))
        .env("RATCHET_FAKE_CLI_LOG", log.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let rx = stdout_lines(&mut child);
    assert!(silent(&rx), "nothing is printed before the first message");

    let user = |text: &str| json!({"type":"user","message":{"role":"user","content":text}});
    let is_result = |v: &Value| v["type"] == "result";

    send(&mut child, user("hello"));
    let turn1 = read_until(&rx, is_result);
    assert_eq!(
        turn1[0]["subtype"], "init",
        "init comes with the first turn"
    );
    assert!(silent(&rx), "the second turn waits for the second message");

    send(&mut child, user("and now?"));
    let turn2 = read_until(&rx, is_result);
    assert!(turn2.iter().all(|v| v["subtype"] != "init"));
    assert_eq!(turn2.last().unwrap()["num_turns"], 2);

    // The same lines, fed to the parser as they came, give both turns.
    let mut p = ClaudeStreamParser::new(|| 1);
    for v in turn1.iter().chain(&turn2) {
        p.parse_line(&v.to_string());
    }
    assert_eq!(p.session_id.as_deref(), Some(CLAUDE_SID));
    finish(child);
    let logged = std::fs::read_to_string(log.path()).unwrap();
    assert_eq!(logged.lines().count(), 2);
}

#[test]
fn fake_codex_app_server_answers_requests_with_their_own_ids_and_streams_a_turn() {
    let mut child = fake_cli("codex-app-server", "codex-0.166.0-mock-session.jsonl", &[]);
    let rx = stdout_lines(&mut child);
    assert!(silent(&rx));

    send(
        &mut child,
        json!({"id":1,"method":"initialize","params":{"clientInfo":{"name":"ratchet","version":"0"}}}),
    );
    let init = read_until(&rx, |v| v.get("id").is_some());
    assert_eq!(init[0]["id"], 1);
    send(&mut child, json!({"method":"initialized","params":{}}));
    assert!(silent(&rx), "client notifications release nothing");

    send(
        &mut child,
        json!({"id":"t-2","method":"thread/start","params":{"cwd":"/work","sandbox":"read-only"}}),
    );
    let thread = read_until(&rx, |v| v["method"] == "thread/started");
    assert_eq!(thread[0]["id"], "t-2");
    assert_eq!(
        thread[0]["result"]["thread"]["id"],
        "0199b3a0-5c1e-7a40-9d2e-6f1a2b3c4d5e"
    );

    let mut spoken = Vec::new();
    for (id, text) in [(3, "hello"), (4, "and now?")] {
        send(
            &mut child,
            json!({"id":id,"method":"turn/start","params":{"threadId":"x","input":[{"type":"text","text":text}]}}),
        );
        let turn = read_until(&rx, |v| v["method"] == "turn/completed");
        assert_eq!(
            turn[0]["id"], id,
            "the response comes first and echoes the id"
        );
        let deltas: String = turn
            .iter()
            .filter(|v| v["method"] == "item/agentMessage/delta")
            .map(|v| v["params"]["delta"].as_str().unwrap())
            .collect();
        spoken.push(deltas);
    }
    assert_eq!(
        spoken,
        [
            "Hello. Ask me about your optimizations.",
            "The newest one is paused."
        ]
    );
    finish(child);
}

#[test]
fn fake_grok_acp_streams_chunks_before_the_prompt_response() {
    let mut child = fake_cli("grok-acp", "grok-1.0.70-mock-session.jsonl", &[]);
    let rx = stdout_lines(&mut child);
    assert!(silent(&rx));

    for (id, method) in [(1, "initialize"), (2, "authenticate"), (3, "session/new")] {
        send(
            &mut child,
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":{}}),
        );
        let answer = read_until(&rx, |v| v.get("id").is_some());
        assert_eq!(answer.len(), 1);
        assert_eq!(answer[0]["id"], id);
    }
    let mut spoken = Vec::new();
    for id in [4, 5] {
        send(
            &mut child,
            json!({"jsonrpc":"2.0","id":id,"method":"session/prompt","params":{"sessionId":"s","prompt":[{"type":"text","text":"hi"}]}}),
        );
        let turn = read_until(&rx, |v| v.get("id").is_some());
        assert_eq!(turn.last().unwrap()["id"], id);
        assert_eq!(turn.last().unwrap()["result"]["stopReason"], "end_turn");
        spoken.push(
            turn.iter()
                .filter(|v| v["method"] == "session/update")
                .map(|v| v["params"]["update"]["content"]["text"].as_str().unwrap())
                .collect::<String>(),
        );
    }
    assert_eq!(
        spoken,
        [
            "Hello. Ask me about your optimizations.",
            "The newest one is paused."
        ]
    );
    finish(child);
}
