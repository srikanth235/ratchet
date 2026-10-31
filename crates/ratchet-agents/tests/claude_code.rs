mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{collect, env_of, fixture};
use ratchet_agents::domain::{AgentAdapter, AgentError, AgentEvent, AgentEventType, AgentKind, AgentSession, AgentSpec, AgentUsage};
use ratchet_agents::process::KillSignal;
use ratchet_agents::testing::{FakeProcessSpec, FakeSpawner};
use ratchet_agents::{ClaudeCodeAdapter, ClaudeCodeOptions, ClaudePermissionMode, ClaudeStreamParser, CliAgentOptions, PromptVia};
use serde_json::json;
use tokio_util::sync::CancellationToken;

const SID: &str = "5f1c1b8e-2d7a-4a37-9c0e-7b6f3b1d0a11";

fn spec() -> AgentSpec {
    AgentSpec { workdir: "/work/agent-1".into(), prompt: "make it faster".into(), ..AgentSpec::default() }
}

fn parse(name: &str, chunk_size: Option<usize>) -> (ClaudeStreamParser, Vec<AgentEvent>) {
    let mut parser = ClaudeStreamParser::new(|| 1);
    let text = fixture(name);
    let mut events = match chunk_size {
        None => parser.push(&text),
        Some(n) => {
            let chars: Vec<char> = text.chars().collect();
            chars.chunks(n).flat_map(|c| parser.push(&c.iter().collect::<String>())).collect()
        }
    };
    events.extend(parser.end());
    (parser, events)
}

fn kinds(events: &[AgentEvent]) -> Vec<AgentEventType> {
    events.iter().map(|e| e.kind).collect()
}

#[test]
fn parser_maps_a_full_transcript_to_status_message_tool_usage_events() {
    use AgentEventType::*;
    let (parser, events) = parse("claude-success.jsonl", None);
    assert_eq!(parser.session_id.as_deref(), Some(SID));
    assert_eq!(parser.model.as_deref(), Some("claude-sonnet-5-5"));
    assert_eq!(kinds(&events), vec![Status, Message, Tool, Tool, Tool, Tool, Message, Usage]);
    let init = events[0].data.as_ref().unwrap();
    assert_eq!(init["sessionId"], json!(SID));
    assert_eq!(init["apiKeySource"], json!("ANTHROPIC_API_KEY"));
    assert_eq!(init["tools"], json!(8));
    assert_eq!(events[2].text.as_deref(), Some("Bash: node --cpu-prof dist/src/main.js"));
    assert_eq!(events[2].data.as_ref().unwrap()["phase"], json!("call"));
    assert_eq!(events[2].data.as_ref().unwrap()["name"], json!("Bash"));
    let result = events[3].data.as_ref().unwrap();
    assert_eq!((&result["phase"], &result["id"], &result["isError"]), (&json!("result"), &json!("toolu_01X"), &json!(false)));
    assert!(events[4].text.as_deref().unwrap().contains("Edit: /work/agent-1/src/search.ts")); // thinking block skipped
    assert!(events[5].text.as_deref().unwrap().contains("has been updated")); // array-form tool_result content
    // input = input + cache creation + cache read, from the result line only (no double counting)
    assert_eq!(events[7].usage, Some(AgentUsage { input_tokens: 16 + 12385 + 36131, output_tokens: 308, cost_usd: Some(0.0841) }));
    let result = parser.result.unwrap();
    assert_eq!((result.subtype.as_str(), result.is_error, result.num_turns), ("success", false, 4));
}

#[test]
fn parser_is_identical_however_the_stream_is_chunked_including_one_char_chunks() {
    let (_, whole) = parse("claude-success.jsonl", None);
    for size in [1, 7, 64, 1000] {
        assert_eq!(parse("claude-success.jsonl", Some(size)).1, whole, "chunk size {size}");
    }
}

#[test]
fn parser_holds_a_partial_last_line_and_parses_an_unterminated_final_line_on_end() {
    let mut parser = ClaudeStreamParser::new(|| 1);
    let line = json!({"type": "assistant", "message": {"content": [{"type": "text", "text": "hi"}]}}).to_string();
    assert!(parser.push(&line[..20]).is_empty());
    assert!(parser.push(&line[20..]).is_empty()); // no newline yet
    assert_eq!(parser.end(), vec![AgentEvent::new(1, AgentEventType::Message).with_text("hi")]);
}

#[test]
fn parser_skips_unknown_event_and_block_types_and_garbage_lines() {
    use AgentEventType::*;
    let (parser, events) = parse("claude-noisy.jsonl", None);
    assert_eq!(kinds(&events), vec![Status, Message, Tool, Usage]);
    assert_eq!(parser.malformed_lines, 3); // stdout noise, the array, the object without type
    assert_eq!(
        parser.unknown_types.iter().map(String::as_str).collect::<Vec<_>>(),
        vec!["block:server_tool_use", "future_event_kind", "rate_limit_event", "stream_event", "system:hook_started"]
    );
    assert_eq!(parser.session_id.as_deref(), Some("sess-noisy"));
}

#[test]
fn parser_reports_error_results_as_error_events_and_keeps_the_usage() {
    use AgentEventType::*;
    let (parser, events) = parse("claude-error-execution.jsonl", None);
    assert_eq!(kinds(&events), vec![Status, Usage, Error]);
    assert!(events[2].text.as_deref().unwrap().contains("authentication_error"));
    let result = parser.result.unwrap();
    assert!(result.is_error);
    assert_eq!(result.subtype, "error_during_execution");
}

#[test]
fn parser_treats_error_max_turns_as_an_error_subtype_result() {
    let (parser, _) = parse("claude-error-max-turns.jsonl", None);
    let result = parser.result.unwrap();
    assert!(result.is_error);
    assert_eq!(result.subtype, "error_max_turns");
}

fn adapter_for(spec: FakeProcessSpec, env: &[(&str, &str)]) -> (FakeSpawner, ClaudeCodeAdapter) {
    let fake = FakeSpawner::new(spec);
    let adapter = ClaudeCodeAdapter::new(ClaudeCodeOptions {
        cli: CliAgentOptions {
            spawner: Some(Arc::new(fake.clone())),
            env: Some(env_of(env)),
            kill_grace: Some(Duration::from_millis(20)),
            ..CliAgentOptions::default()
        },
        ..ClaudeCodeOptions::default()
    });
    (fake, adapter)
}

const KEYED: [(&str, &str); 2] = [("ANTHROPIC_API_KEY", "sk-test"), ("PATH", "/bin")];

async fn start(adapter: &ClaudeCodeAdapter, spec: &AgentSpec, on_event: impl Fn(AgentEvent) + Send + Sync) -> Result<AgentSession, AgentError> {
    adapter.start(spec, &on_event, &CancellationToken::new()).await
}

#[tokio::test]
async fn start_builds_the_command_line_pipes_the_prompt_on_stdin_and_returns_the_session() {
    let (fake, adapter) = adapter_for(FakeProcessSpec::stdout(fixture("claude-success.jsonl")).with_chunk_size(50), &KEYED);
    let (events, on_event) = collect();
    let mut s = spec();
    s.model = Some("claude-opus-5-5".into());
    s.max_turns = Some(40);
    s.env = Some(env_of(&[("FOO", "bar")]));
    let session = start(&adapter, &s, on_event).await.unwrap();
    assert_eq!(session, AgentSession { id: SID.into(), kind: AgentKind::ClaudeCode });
    let call = fake.call(0);
    assert_eq!(call.command, "claude");
    assert_eq!(
        call.args,
        [
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-mode",
            "acceptEdits",
            "--allowedTools",
            "Bash,Read,Edit,Write,Glob,Grep",
            "--model",
            "claude-opus-5-5",
            "--max-turns",
            "40"
        ]
    );
    assert_eq!(call.stdin, "make it faster");
    assert_eq!(call.options.cwd.to_str(), Some("/work/agent-1"));
    assert_eq!(call.options.env["ANTHROPIC_API_KEY"], "sk-test");
    assert_eq!(call.options.env["FOO"], "bar");
    assert!(events.lock().unwrap().iter().any(|e| e.kind == AgentEventType::Usage));
}

#[tokio::test]
async fn honours_binary_path_permission_mode_tool_lists_and_prompt_as_argument() {
    let fake = FakeSpawner::new(FakeProcessSpec::stdout(fixture("claude-success.jsonl")));
    let adapter = ClaudeCodeAdapter::new(ClaudeCodeOptions {
        cli: CliAgentOptions {
            spawner: Some(Arc::new(fake.clone())),
            env: Some(env_of(&[])),
            binary: Some("/opt/claude/bin/claude".into()),
            extra_args: vec!["--add-dir".into(), "/data".into()],
            ..CliAgentOptions::default()
        },
        api_key: Some("sk-from-option".into()),
        permission_mode: Some(ClaudePermissionMode::BypassPermissions),
        allowed_tools: Some(vec!["Bash(git:*)".into(), "Edit".into()]),
        disallowed_tools: vec!["WebFetch".into()],
        prompt_via: PromptVia::Arg,
        ..ClaudeCodeOptions::default()
    });
    start(&adapter, &spec(), |_| {}).await.unwrap();
    let call = fake.call(0);
    assert_eq!(call.command, "/opt/claude/bin/claude");
    assert_eq!(
        call.args,
        [
            "-p",
            "--output-format",
            "stream-json",
            "--verbose",
            "--permission-mode",
            "bypassPermissions",
            "--allowedTools",
            "Bash(git:*),Edit",
            "--disallowedTools",
            "WebFetch",
            "--add-dir",
            "/data",
            "--",
            "make it faster"
        ]
    );
    assert_eq!(call.stdin, "");
    assert_eq!(call.options.env["ANTHROPIC_API_KEY"], "sk-from-option");
}

#[tokio::test]
async fn resume_passes_resume_id_sends_the_message_and_keeps_the_session_id() {
    let (fake, adapter) = adapter_for(FakeProcessSpec::stdout(fixture("claude-resume.jsonl")), &KEYED);
    let mut session = AgentSession { id: SID.into(), kind: AgentKind::ClaudeCode };
    let (events, on_event) = collect();
    adapter.resume(&mut session, &spec(), "verdict: verified", &on_event, &CancellationToken::new()).await.unwrap();
    let call = fake.call(0);
    let at = call.args.iter().position(|a| a == "--resume").expect("--resume present");
    assert_eq!(call.args[at + 1], SID);
    assert_eq!(call.stdin, "verdict: verified");
    assert_eq!(session.id, SID);
    let usage = events.lock().unwrap().iter().find(|e| e.kind == AgentEventType::Usage).unwrap().usage;
    assert_eq!(usage, Some(AgentUsage { input_tokens: 12606, output_tokens: 120, cost_usd: Some(0.0312) }));
}

#[tokio::test]
async fn resume_follows_a_new_session_id_if_the_cli_forks_one() {
    let (_, adapter) = adapter_for(FakeProcessSpec::stdout(fixture("claude-resume-new-id.jsonl")), &KEYED);
    let mut session = AgentSession { id: SID.into(), kind: AgentKind::ClaudeCode };
    adapter.resume(&mut session, &spec(), "go on", &|_| {}, &CancellationToken::new()).await.unwrap();
    assert_eq!(session.id, "0b9d4c20-aaaa-bbbb-cccc-111122223333");
}

#[tokio::test]
async fn resume_of_an_unknown_session_turns_a_non_zero_exit_with_stderr_into_an_error_and_an_error_event() {
    let (_, adapter) = adapter_for(FakeProcessSpec::exit(1).with_stderr("No conversation found with session ID: nope\n"), &KEYED);
    let (events, on_event) = collect();
    let mut session = AgentSession { id: "nope".into(), kind: AgentKind::ClaudeCode };
    let err = adapter.resume(&mut session, &spec(), "x", &on_event, &CancellationToken::new()).await.unwrap_err();
    match err {
        AgentError::Failed { message, exit_code, .. } => {
            assert!(message.contains("No conversation found"), "{message}");
            assert_eq!(exit_code, Some(1));
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(events.lock().unwrap().last().unwrap().kind, AgentEventType::Error);
}

#[tokio::test]
async fn an_is_error_result_fails_the_turn_with_the_error_event_emitted_once() {
    let (_, adapter) = adapter_for(FakeProcessSpec::stdout(fixture("claude-error-execution.jsonl")).with_exit(1), &KEYED);
    let (events, on_event) = collect();
    let err = start(&adapter, &spec(), on_event).await.unwrap_err();
    assert!(err.to_string().contains("authentication_error"), "{err}");
    assert_eq!(events.lock().unwrap().iter().filter(|e| e.kind == AgentEventType::Error).count(), 1);
}

#[tokio::test]
async fn error_max_turns_ends_the_turn_normally_so_the_runner_can_carry_on() {
    let (_, adapter) = adapter_for(FakeProcessSpec::stdout(fixture("claude-error-max-turns.jsonl")), &KEYED);
    let session = start(&adapter, &spec(), |_| {}).await.unwrap();
    assert_eq!(session.id, "sess-turns");
}

#[tokio::test]
async fn a_stream_that_ends_without_a_result_line_is_an_error_even_with_exit_zero() {
    let head = fixture("claude-success.jsonl").split('\n').take(3).collect::<Vec<_>>().join("\n");
    let (_, adapter) = adapter_for(FakeProcessSpec::stdout(format!("{head}\n{{\"type\":\"assis")), &KEYED);
    let err = start(&adapter, &spec(), |_| {}).await.unwrap_err();
    assert!(err.to_string().contains("without a result"), "{err}");
}

#[tokio::test]
async fn a_missing_binary_is_reported_not_raised_raw() {
    let (_, adapter) = adapter_for(FakeProcessSpec::spawn_error("spawn claude ENOENT"), &KEYED);
    let err = start(&adapter, &spec(), |_| {}).await.unwrap_err().to_string();
    assert!(err.contains("cannot run 'claude'") && err.contains("ENOENT"), "{err}");
}

#[tokio::test]
async fn abort_signals_the_process_group_with_sigterm_and_fails_with_aborted() {
    let head = fixture("claude-success.jsonl").split('\n').take(2).collect::<Vec<_>>().join("\n") + "\n";
    let (fake, adapter) = adapter_for(FakeProcessSpec::stdout(head).hanging(), &KEYED);
    let cancel = CancellationToken::new();
    let (events, collect_event) = collect();
    let trigger = cancel.clone();
    let on_event = move |e: AgentEvent| {
        let is_status = e.kind == AgentEventType::Status;
        collect_event(e);
        if is_status {
            trigger.cancel(); // abort once the session has started
        }
    };
    let err = adapter.start(&spec(), &on_event, &cancel).await.unwrap_err();
    match err {
        AgentError::Aborted { session } => assert_eq!(session.map(|s| s.id), Some(SID.to_string())),
        other => panic!("{other:?}"),
    }
    assert_eq!(fake.call(0).signals[0], KillSignal::Term);
    let events = events.lock().unwrap();
    let last = events.last().unwrap();
    assert_eq!((last.kind, last.text.as_deref()), (AgentEventType::Status, Some("claude-code turn aborted")));
}

#[tokio::test]
async fn an_already_aborted_signal_never_lets_the_turn_proceed() {
    let (fake, adapter) = adapter_for(FakeProcessSpec::hang(), &KEYED);
    let cancel = CancellationToken::new();
    cancel.cancel();
    let err = adapter.start(&spec(), &|_| {}, &cancel).await.unwrap_err();
    assert!(err.is_aborted());
    assert!(fake.call(0).signals.contains(&KillSignal::Term));
}

#[tokio::test]
async fn preflight_is_ok_with_the_binary_present_and_a_key() {
    let (_, adapter) = adapter_for(FakeProcessSpec::stdout("2.1.0 (Claude Code)\n"), &KEYED);
    let r = adapter.preflight().await;
    assert!(r.ok && r.problems.is_empty());
}

#[tokio::test]
async fn preflight_reports_a_missing_key_and_a_missing_binary() {
    let (_, adapter) = adapter_for(FakeProcessSpec::spawn_error("ENOENT"), &[]);
    let r = adapter.preflight().await;
    assert!(!r.ok);
    assert_eq!(r.problems.len(), 2);
    assert!(r.problems[0].contains("cannot run 'claude'"));
    assert_eq!(r.problems[1], "ANTHROPIC_API_KEY is not set");
}

#[tokio::test]
async fn preflight_allows_a_subscription_login_explicitly() {
    let fake = FakeSpawner::new(FakeProcessSpec::stdout("2.1.0\n"));
    let adapter = ClaudeCodeAdapter::new(ClaudeCodeOptions {
        cli: CliAgentOptions { spawner: Some(Arc::new(fake)), env: Some(env_of(&[])), ..CliAgentOptions::default() },
        allow_subscription_auth: true,
        ..ClaudeCodeOptions::default()
    });
    assert!(adapter.preflight().await.ok);
}
