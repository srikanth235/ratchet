mod common;

use ratchet_core::enums::AgentKind;
use ratchet_core::ports::{
    AgentAdapter, AgentEvent, AgentEventType, AgentSession, AgentSpec, AgentUsage,
    CancellationToken, PortError,
};
use std::sync::Arc;
use std::time::Duration;

use common::{collect, env_of, fixture};

use ratchet_agents::process::KillSignal;
use ratchet_agents::testing::{FakeProcessSpec, FakeSpawner};
use ratchet_agents::{CliAgentOptions, GrokAdapter, GrokOptions, GrokStreamParser};
use serde_json::json;

const SID: &str = "01a195ae-1291-7af1-8f7b-691a16d61c18";

fn spec() -> AgentSpec {
    AgentSpec {
        workdir: "/work/agent-2".into(),
        prompt: "make it faster".into(),
        ..AgentSpec::default()
    }
}

fn parse(name: &str, chunk_size: Option<usize>) -> (GrokStreamParser, Vec<AgentEvent>) {
    let mut parser = GrokStreamParser::new(|| 1);
    let text = fixture(name);
    let mut events = match chunk_size {
        None => parser.push(&text),
        Some(n) => {
            let chars: Vec<char> = text.chars().collect();
            chars
                .chunks(n)
                .flat_map(|c| parser.push(&c.iter().collect::<String>()))
                .collect()
        }
    };
    events.extend(parser.end());
    (parser, events)
}

fn label(e: &AgentEvent) -> String {
    let kind = format!("{:?}", e.event_type).to_lowercase();
    match e
        .data
        .as_ref()
        .and_then(|d| d.get("phase"))
        .and_then(|p| p.as_str())
    {
        Some(phase) => format!("{kind}:{phase}"),
        None => kind,
    }
}

#[test]
fn parser_joins_text_deltas_into_messages_and_maps_tool_calls_and_the_end_line_to_usage() {
    let (parser, events) = parse("grok-success.jsonl", None);
    assert_eq!(parser.session_id.as_deref(), Some(SID));
    assert_eq!(
        events.iter().map(label).collect::<Vec<_>>(),
        [
            "message",
            "tool:call",
            "tool:result",
            "message",
            "tool:call",
            "tool:result",
            "tool:call",
            "tool:result",
            "message",
            "usage"
        ]
    );
    assert_eq!(events[0].text.as_deref(), Some("Let me profile it."));
    assert_eq!(
        events[1].text.as_deref(),
        Some("run_terminal_command: node --cpu-prof dist/src/main.js")
    );
    assert_eq!(events[1].data.as_ref().unwrap()["id"], json!("call-1"));
    let ok = events[2].data.as_ref().unwrap();
    assert_eq!(
        (&ok["exitCode"], &ok["isError"]),
        (&json!(0), &json!(false))
    );
    assert_eq!(events[2].text.as_deref(), Some("hot: filter()\n"));
    assert_eq!(
        events[4].text.as_deref(),
        Some("search_replace: /work/agent-2/src/filter.ts")
    );
    let edit = events[5].data.as_ref().unwrap();
    assert_eq!(
        (&edit["exitCode"], &edit["isError"]),
        (&json!(null), &json!(false))
    );
    let failed = events[7].data.as_ref().unwrap();
    assert_eq!(
        (&failed["exitCode"], &failed["isError"]),
        (&json!(1), &json!(true))
    );
    assert_eq!(events[8].text.as_deref(), Some("Tests fail, reverting."));
    // Cache reads and writes count as input; the dollars come from the CLI.
    assert_eq!(
        events[9].usage,
        Some(AgentUsage {
            input_tokens: 26124 + 45952 + 100,
            output_tokens: 410,
            cost_usd: Some(0.155368)
        })
    );
    assert!(parser.turn.completed && !parser.turn.failed);
}

#[test]
fn parser_is_chunking_independent() {
    let (_, whole) = parse("grok-success.jsonl", None);
    for size in [1, 13, 100] {
        assert_eq!(
            parse("grok-success.jsonl", Some(size)).1,
            whole,
            "chunk size {size}"
        );
    }
}

#[test]
fn parser_tolerates_unknown_events_and_stdout_noise() {
    let (parser, events) = parse("grok-noisy.jsonl", None);
    assert_eq!(
        events.iter().map(label).collect::<Vec<_>>(),
        ["tool:call", "tool:result", "message", "usage"]
    );
    assert_eq!(parser.malformed_lines, 1);
    assert_eq!(
        parser
            .unknown_types
            .iter()
            .map(String::as_str)
            .collect::<Vec<_>>(),
        vec!["something.new"]
    );
    assert!(parser.turn.completed && !parser.turn.failed);
}

#[test]
fn parser_a_stop_reason_other_than_end_turn_fails_the_turn() {
    let (parser, events) = parse("grok-stopped.jsonl", None);
    assert!(parser.turn.failed && !parser.turn.completed);
    assert!(parser.turn.error.as_deref().unwrap().contains("max_turns"));
    assert_eq!(parser.session_id.as_deref(), Some(SID));
    assert_eq!(events.last().unwrap().event_type, AgentEventType::Error);
}

#[test]
fn parser_error_line_fails_the_turn_with_the_vendor_message() {
    let (parser, _) = parse("grok-error.jsonl", None);
    assert!(parser.turn.failed && !parser.turn.completed);
    assert!(parser.turn.error.as_deref().unwrap().contains("429"));
}

fn adapter_for(plan: FakeProcessSpec, env: &[(&str, &str)]) -> (FakeSpawner, GrokAdapter) {
    let fake = FakeSpawner::new(plan);
    let adapter = GrokAdapter::new(GrokOptions {
        cli: CliAgentOptions {
            spawner: Some(Arc::new(fake.clone())),
            env: Some(env_of(env)),
            kill_grace: Some(Duration::from_millis(20)),
            ..CliAgentOptions::default()
        },
        ..GrokOptions::default()
    });
    (fake, adapter)
}

const KEYED: [(&str, &str); 2] = [("XAI_API_KEY", "xai-key"), ("PATH", "/bin")];

#[tokio::test]
async fn start_runs_grok_headless_with_the_prompt_on_stdin_and_returns_the_session_id() {
    let (fake, adapter) = adapter_for(
        FakeProcessSpec::stdout(fixture("grok-success.jsonl")).with_chunk_size(40),
        &KEYED,
    );
    let (events, on_event) = collect();
    let mut s = spec();
    s.model = Some("grok-4.7".into());
    let session = adapter
        .start(&s, &on_event, Some(&CancellationToken::new()))
        .await
        .unwrap();
    assert_eq!(
        session,
        AgentSession {
            id: SID.into(),
            kind: AgentKind::Grok
        }
    );
    let call = fake.call(0);
    assert_eq!(call.command, "grok");
    assert_eq!(
        call.args,
        [
            "-p",
            "-",
            "--output-format",
            "streaming-json",
            "--permission-mode",
            "acceptEdits",
            "--model",
            "grok-4.7"
        ]
    );
    assert_eq!(call.stdin, "make it faster");
    assert_eq!(call.options.cwd.to_str(), Some("/work/agent-2"));
    assert_eq!(call.options.env["XAI_API_KEY"], "xai-key");
    assert_eq!(
        events.lock().unwrap().last().unwrap().event_type,
        AgentEventType::Usage
    );
}

#[tokio::test]
async fn resume_passes_the_session_id_to_resume() {
    let (fake, adapter) = adapter_for(
        FakeProcessSpec::stdout(fixture("grok-resume.jsonl")),
        &KEYED,
    );
    let mut session = AgentSession {
        id: SID.into(),
        kind: AgentKind::Grok,
    };
    let (events, on_event) = collect();
    adapter
        .resume(
            &mut session,
            &spec(),
            "verdict: verified",
            &on_event,
            Some(&CancellationToken::new()),
        )
        .await
        .unwrap();
    let call = fake.call(0);
    assert_eq!(call.args[..2], ["-p", "-"]);
    let at = call.args.iter().position(|a| a == "--resume").unwrap();
    assert_eq!(call.args[at + 1], SID);
    assert_eq!(call.stdin, "verdict: verified");
    let events = events.lock().unwrap();
    assert!(
        events
            .iter()
            .find(|e| e.event_type == AgentEventType::Message)
            .unwrap()
            .text
            .as_deref()
            .unwrap()
            .contains("Verdict verified")
    );
}

#[tokio::test]
async fn permission_mode_sandbox_and_turn_limit_are_configurable() {
    let fake = FakeSpawner::new(FakeProcessSpec::stdout(fixture("grok-success.jsonl")));
    let adapter = GrokAdapter::new(GrokOptions {
        cli: CliAgentOptions {
            spawner: Some(Arc::new(fake.clone())),
            env: Some(env_of(&[])),
            binary: Some("/usr/local/bin/grok".into()),
            extra_args: vec!["--disable-web-search".into()],
            ..CliAgentOptions::default()
        },
        api_key: Some("k".into()),
        permission_mode: Some("bypassPermissions".into()),
        sandbox: Some("strict".into()),
        max_turns: Some(40),
        ..GrokOptions::default()
    });
    adapter
        .start(&spec(), &|_| {}, Some(&CancellationToken::new()))
        .await
        .unwrap();
    let call = fake.call(0);
    assert_eq!(call.command, "/usr/local/bin/grok");
    assert_eq!(
        call.args,
        [
            "-p",
            "-",
            "--output-format",
            "streaming-json",
            "--permission-mode",
            "bypassPermissions",
            "--sandbox",
            "strict",
            "--max-turns",
            "40",
            "--disable-web-search"
        ]
    );
    assert_eq!(call.options.env["XAI_API_KEY"], "k");
}

#[tokio::test]
async fn an_error_line_becomes_an_error_carrying_the_vendor_message() {
    let (_, adapter) = adapter_for(
        FakeProcessSpec::stdout(fixture("grok-error.jsonl")).with_exit(1),
        &KEYED,
    );
    let (events, on_event) = collect();
    let err = adapter
        .start(&spec(), &on_event, Some(&CancellationToken::new()))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("429"), "{err}");
    assert!(
        events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.event_type == AgentEventType::Error)
    );
}

#[tokio::test]
async fn a_stop_reason_other_than_end_turn_fails_the_agent() {
    let (_, adapter) = adapter_for(
        FakeProcessSpec::stdout(fixture("grok-stopped.jsonl")),
        &KEYED,
    );
    let err = adapter
        .start(&spec(), &|_| {}, Some(&CancellationToken::new()))
        .await
        .unwrap_err();
    assert!(err.to_string().contains("max_turns"), "{err}");
}

#[tokio::test]
async fn non_zero_exit_without_an_end_event_carries_stderr() {
    let (_, adapter) = adapter_for(
        FakeProcessSpec::exit(2).with_stderr("error: unknown session\n"),
        &KEYED,
    );
    let mut session = AgentSession {
        id: "x".into(),
        kind: AgentKind::Grok,
    };
    let err = adapter
        .resume(
            &mut session,
            &spec(),
            "m",
            &|_| {},
            Some(&CancellationToken::new()),
        )
        .await
        .unwrap_err();
    assert!(matches!(err, PortError::AgentFailed { .. }));
    assert!(err.to_string().contains("unknown session"), "{err}");
}

#[tokio::test]
async fn abort_kills_the_process_group_and_fails_with_aborted() {
    let first = fixture("grok-success.jsonl")
        .split('\n')
        .take(6)
        .collect::<Vec<_>>()
        .join("\n")
        + "\n";
    let (fake, adapter) = adapter_for(FakeProcessSpec::stdout(first).hanging(), &KEYED);
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let on_event = move |e: AgentEvent| {
        if e.event_type == AgentEventType::Tool {
            trigger.cancel();
        }
    };
    let err = adapter
        .start(&spec(), &on_event, Some(&cancel))
        .await
        .unwrap_err();
    assert!(err.is_cancelled());
    assert_eq!(fake.call(0).signals, vec![KillSignal::Term]);
}

#[tokio::test]
async fn preflight_checks_key_and_binary() {
    assert!(
        adapter_for(
            FakeProcessSpec::stdout("grok 1.0.67 (2fe064390d07)\n"),
            &KEYED
        )
        .1
        .preflight()
        .await
        .unwrap()
        .ok
    );
    let bad = adapter_for(FakeProcessSpec::spawn_error("ENOENT"), &[])
        .1
        .preflight()
        .await
        .unwrap();
    assert_eq!(bad.problems.len(), 2);
}
