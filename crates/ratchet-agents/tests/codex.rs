mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{collect, env_of, fixture};
use ratchet_agents::domain::{AgentAdapter, AgentError, AgentEvent, AgentEventType, AgentKind, AgentSession, AgentSpec, AgentUsage};
use ratchet_agents::process::KillSignal;
use ratchet_agents::testing::{FakeProcessSpec, FakeSpawner};
use ratchet_agents::{CliAgentOptions, CodexAdapter, CodexOptions, CodexPricing, CodexSandbox, CodexStreamParser, PromptVia};
use serde_json::json;
use tokio_util::sync::CancellationToken;

const TID: &str = "0199a213-81c0-7800-8aa1-bbab2a035a53";

fn spec() -> AgentSpec {
    AgentSpec { workdir: "/work/agent-2".into(), prompt: "make it faster".into(), ..AgentSpec::default() }
}

fn parse(name: &str, chunk_size: Option<usize>, pricing: Option<CodexPricing>) -> (CodexStreamParser, Vec<AgentEvent>) {
    let mut parser = CodexStreamParser::new(|| 1, pricing);
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

fn label(e: &AgentEvent) -> String {
    let kind = format!("{:?}", e.kind).to_lowercase();
    match e.data.as_ref().and_then(|d| d.get("phase")).and_then(|p| p.as_str()) {
        Some(phase) => format!("{kind}:{phase}"),
        None => kind,
    }
}

#[test]
fn parser_maps_items_to_message_and_tool_events_and_turn_completed_to_usage() {
    let (parser, events) = parse("codex-success.jsonl", None, None);
    assert_eq!(parser.thread_id.as_deref(), Some(TID));
    assert_eq!(
        events.iter().map(label).collect::<Vec<_>>(),
        ["status", "tool:call", "tool:result", "message", "tool:call", "tool:call", "tool:result", "message", "usage"]
    );
    assert_eq!(events[1].text.as_deref(), Some("command: bash -lc 'node --cpu-prof dist/src/main.js'"));
    let ok = events[2].data.as_ref().unwrap();
    assert_eq!((&ok["exitCode"], &ok["isError"]), (&json!(0), &json!(false)));
    assert_eq!(events[4].text.as_deref(), Some("apply_patch: /work/agent-2/src/filter.ts"));
    let failed = events[6].data.as_ref().unwrap();
    assert_eq!((&failed["exitCode"], &failed["isError"]), (&json!(1), &json!(true)));
    assert_eq!(events[8].usage, Some(AgentUsage { input_tokens: 24763, output_tokens: 1122, cost_usd: None })); // no cost without a price book
    assert_eq!(events[8].data.as_ref().unwrap()["cachedInputTokens"], json!(24448));
}

#[test]
fn parser_estimates_cost_when_pricing_is_given_with_cached_input_at_the_cached_rate() {
    let pricing = CodexPricing { input_per_m: 1.25, cached_input_per_m: Some(0.125), output_per_m: 10.0 };
    let (_, events) = parse("codex-success.jsonl", None, Some(pricing));
    let cost = events.iter().find(|e| e.kind == AgentEventType::Usage).unwrap().usage.unwrap().cost_usd.unwrap();
    let expected = ((24763.0 - 24448.0) * 1.25 + 24448.0 * 0.125 + 1122.0 * 10.0) / 1e6;
    assert!((cost - expected).abs() < 1e-10, "{cost} vs {expected}");
}

#[test]
fn parser_is_chunking_independent() {
    let (_, whole) = parse("codex-success.jsonl", None, None);
    for size in [1, 13, 100] {
        assert_eq!(parse("codex-success.jsonl", Some(size), None).1, whole, "chunk size {size}");
    }
}

#[test]
fn parser_tolerates_unknown_events_items_and_stdout_noise_and_non_fatal_errors_do_not_fail_the_turn() {
    let (parser, events) = parse("codex-noisy.jsonl", None, None);
    assert_eq!(events.iter().map(|e| e.kind).collect::<Vec<_>>(), {
        use AgentEventType::*;
        vec![Status, Error, Tool, Message, Usage]
    });
    assert_eq!(events[1].data.as_ref().unwrap().get("fatal"), Some(&json!(false)));
    assert_eq!(parser.malformed_lines, 1);
    assert_eq!(parser.unknown_types.iter().map(String::as_str).collect::<Vec<_>>(), vec!["item:hologram", "something.new"]);
    assert!(parser.turn.completed && !parser.turn.failed);
}

#[test]
fn parser_turn_failed_marks_the_turn_failed_with_the_vendor_message() {
    let (parser, _) = parse("codex-failed.jsonl", None, None);
    assert!(parser.turn.failed && !parser.turn.completed);
    assert!(parser.turn.error.as_deref().unwrap().contains("429"));
}

fn adapter_for(plan: FakeProcessSpec, env: &[(&str, &str)]) -> (FakeSpawner, CodexAdapter) {
    let fake = FakeSpawner::new(plan);
    let adapter = CodexAdapter::new(CodexOptions {
        cli: CliAgentOptions {
            spawner: Some(Arc::new(fake.clone())),
            env: Some(env_of(env)),
            kill_grace: Some(Duration::from_millis(20)),
            ..CliAgentOptions::default()
        },
        ..CodexOptions::default()
    });
    (fake, adapter)
}

const KEYED: [(&str, &str); 2] = [("OPENAI_API_KEY", "sk-oai"), ("PATH", "/bin")];

#[tokio::test]
async fn start_runs_codex_exec_json_with_the_prompt_on_stdin_via_dash_and_returns_the_thread_id() {
    let (fake, adapter) = adapter_for(FakeProcessSpec::stdout(fixture("codex-success.jsonl")).with_chunk_size(40), &KEYED);
    let (events, on_event) = collect();
    let mut s = spec();
    s.model = Some("gpt-5-codex".into());
    let session = adapter.start(&s, &on_event, &CancellationToken::new()).await.unwrap();
    assert_eq!(session, AgentSession { id: TID.into(), kind: AgentKind::Codex });
    let call = fake.call(0);
    assert_eq!(call.command, "codex");
    assert_eq!(
        call.args,
        [
            "exec",
            "--json",
            "--skip-git-repo-check",
            "--model",
            "gpt-5-codex",
            "-c",
            "sandbox_mode=\"workspace-write\"",
            "-c",
            "sandbox_workspace_write.network_access=true",
            "-"
        ]
    );
    assert_eq!(call.stdin, "make it faster");
    assert_eq!(call.options.cwd.to_str(), Some("/work/agent-2"));
    assert_eq!(call.options.env["OPENAI_API_KEY"], "sk-oai");
    assert_eq!(call.options.env["CODEX_API_KEY"], "sk-oai");
    assert_eq!(events.lock().unwrap().last().unwrap().kind, AgentEventType::Usage);
}

#[tokio::test]
async fn resume_runs_codex_exec_resume_thread_json() {
    let (fake, adapter) = adapter_for(FakeProcessSpec::stdout(fixture("codex-resume.jsonl")), &KEYED);
    let mut session = AgentSession { id: TID.into(), kind: AgentKind::Codex };
    let (events, on_event) = collect();
    adapter.resume(&mut session, &spec(), "verdict: verified", &on_event, &CancellationToken::new()).await.unwrap();
    let call = fake.call(0);
    assert_eq!(call.args[..4], ["exec", "resume", TID, "--json"]);
    assert_eq!(call.args.last().map(String::as_str), Some("-"));
    assert_eq!(call.stdin, "verdict: verified");
    let events = events.lock().unwrap();
    assert!(events.iter().find(|e| e.kind == AgentEventType::Message).unwrap().text.as_deref().unwrap().contains("Verdict verified"));
}

#[tokio::test]
async fn sandbox_prompt_as_argument_and_extra_overrides_are_configurable() {
    let fake = FakeSpawner::new(FakeProcessSpec::stdout(fixture("codex-success.jsonl")));
    let adapter = CodexAdapter::new(CodexOptions {
        cli: CliAgentOptions { spawner: Some(Arc::new(fake.clone())), env: Some(env_of(&[])), binary: Some("/usr/local/bin/codex".into()), ..CliAgentOptions::default() },
        api_key: Some("k".into()),
        sandbox: Some(CodexSandbox::DangerFullAccess),
        prompt_via: PromptVia::Arg,
        config_overrides: vec!["model_reasoning_effort=\"high\"".into()],
        ..CodexOptions::default()
    });
    adapter.start(&spec(), &|_| {}, &CancellationToken::new()).await.unwrap();
    let call = fake.call(0);
    assert_eq!(call.command, "/usr/local/bin/codex");
    assert_eq!(
        call.args,
        [
            "exec",
            "--json",
            "--skip-git-repo-check",
            "-c",
            "sandbox_mode=\"danger-full-access\"",
            "-c",
            "model_reasoning_effort=\"high\"",
            "--",
            "make it faster"
        ]
    );
    assert_eq!(call.options.env["OPENAI_API_KEY"], "k");
}

#[tokio::test]
async fn turn_failed_becomes_an_error_carrying_the_vendor_message() {
    let (_, adapter) = adapter_for(FakeProcessSpec::stdout(fixture("codex-failed.jsonl")).with_exit(1), &KEYED);
    let (events, on_event) = collect();
    let err = adapter.start(&spec(), &on_event, &CancellationToken::new()).await.unwrap_err();
    assert!(err.to_string().contains("429"), "{err}");
    assert!(events.lock().unwrap().iter().filter(|e| e.kind == AgentEventType::Error).count() > 0);
}

#[tokio::test]
async fn non_zero_exit_without_turn_completed_carries_stderr() {
    let (_, adapter) = adapter_for(FakeProcessSpec::exit(2).with_stderr("error: unknown session\n"), &KEYED);
    let mut session = AgentSession { id: "x".into(), kind: AgentKind::Codex };
    let err = adapter.resume(&mut session, &spec(), "m", &|_| {}, &CancellationToken::new()).await.unwrap_err();
    assert!(matches!(err, AgentError::Failed { .. }));
    assert!(err.to_string().contains("unknown session"), "{err}");
}

#[tokio::test]
async fn abort_kills_the_process_group_and_fails_with_aborted() {
    let first = fixture("codex-success.jsonl").split('\n').take(1).collect::<Vec<_>>().join("\n") + "\n";
    let (fake, adapter) = adapter_for(FakeProcessSpec::stdout(first).hanging(), &KEYED);
    let cancel = CancellationToken::new();
    let trigger = cancel.clone();
    let on_event = move |e: AgentEvent| {
        if e.kind == AgentEventType::Status {
            trigger.cancel();
        }
    };
    let err = adapter.start(&spec(), &on_event, &cancel).await.unwrap_err();
    assert!(err.is_aborted());
    assert_eq!(fake.call(0).signals, vec![KillSignal::Term]);
}

#[tokio::test]
async fn preflight_checks_key_and_binary() {
    assert!(adapter_for(FakeProcessSpec::stdout("codex-cli 0.50.0\n"), &KEYED).1.preflight().await.ok);
    let bad = adapter_for(FakeProcessSpec::spawn_error("ENOENT"), &[]).1.preflight().await;
    assert_eq!(bad.problems.len(), 2);
}
