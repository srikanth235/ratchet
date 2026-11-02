//! The conformance suite: one table of scenarios, run against every harness, in both ways a harness is driven.
//!
//! * **one-shot**: `AgentAdapter::start` / `resume`, one CLI process per turn (what an optimization run does), through
//!   `FakeSpawner` replaying a fixture.
//! * **live**: `Harness::open` + `send`, the chat path. The three real CLIs speak their long-lived protocol to `tests/fake_cli`;
//!   a harness without a live protocol runs the same table as spawn-per-turn through `FakeSpawner`.
//!
//! Scenarios: start, resume, deltas, tool call and result, usage and cost, rate limit, auth expired, crash, interrupt, an MCP
//! tool call, and a read-only refusal. A harness is a [`Subject`] row: how to build it and which fixture plays each scenario.
//!
//! **Adding a harness** takes its adapter and its fixtures (`<cli>-<version>-mock-<behaviour>.jsonl`) plus one `Subject` row in
//! [`subjects`]. No scenario changes. The test-only `echo` harness (`tests/conformance/echo.rs`) is the fourth row and proves it:
//! `a_fourth_harness_passes_the_whole_suite_with_nothing_but_an_adapter_and_fixtures` runs it alone, and the macro below runs it
//! with the others.
//!
//! What the table fixes for every harness (see `docs/harness-matrix.md`):
//!
//! * a rate limit is **one** `rate_limited` event; `data.resetsAt` is epoch **seconds** when the CLI said when the limit lifts
//!   (the fallback layer waits) and `null` when it did not (it switches harness, or cools down);
//! * an expired login is **one** `auth_expired` event and never carries a reset time (the fallback layer always switches);
//! * both end the turn with `PortError::AgentFailed`; an interrupt ends it with `Ok` (live) or `Cancelled` (one-shot);
//! * a crash is an `error` event and `AgentFailed`, never a message and never mistaken for a rate limit.

mod common;
#[path = "conformance/echo.rs"]
mod echo;

use std::future::Future;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ratchet_agents::process::Spawner;
use ratchet_agents::testing::{FakeProcessSpec, FakeSpawner};
use ratchet_agents::{
    ClaudeCodeAdapter, ClaudeCodeOptions, CliAgentOptions, CodexAdapter, CodexOptions, GrokAdapter,
    GrokOptions,
};
use ratchet_core::enums::AgentKind;
use ratchet_core::ports::{
    Access, AgentEvent, AgentEventType, AgentSpec, Harness, LiveSession, McpServer, PortError,
    PortResult, SessionSpec, SharedEventSink,
};
use serde_json::json;
use tokio_util::sync::CancellationToken;

use common::{Redirect, collect, env_of, fixture};

type Events = Arc<Mutex<Vec<AgentEvent>>>;

// ------------------------------------------------------------------------------------------------------------ the table

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Mode {
    OneShot,
    Live,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::OneShot => "one-shot",
            Mode::Live => "live",
        }
    }
}

/// Which fixture plays each scenario in one mode.
#[derive(Clone, Copy)]
struct Fixtures {
    /// A normal turn. Live: the CLI's own two-turn session (two messages, two usages).
    start: &'static str,
    /// The turn after `start`: one-shot, the resumed turn; live, the transcript a reopened (resumed) session replays.
    resume: &'static str,
    deltas: &'static str,
    /// A turn that ends with a usage report (the live Grok session transcript has none).
    usage: &'static str,
    tool: &'static str,
    mcp_tool: &'static str,
    read_only: &'static str,
    rate_limit: &'static str,
    auth_expired: &'static str,
    /// Ends in a `{"$exit":N}` crash directive, mid-turn.
    crash: &'static str,
    /// Live: a transcript with a `{"$hold":true}`; one-shot: the head of it is the output of a process that then hangs.
    interrupt: &'static str,
}

/// When a rate limit lifts, as the CLI said it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Reset {
    /// Not said: the fallback layer cannot wait for it.
    Unknown,
    /// An absolute time, epoch seconds.
    At(i64),
    /// Relative to now ("retry after 30s"): a time within the next hour.
    Soon,
}

/// What a harness promises beyond the shared contract, per mode.
#[derive(Clone, Copy)]
struct Expect {
    /// Partial text arrives as `Delta` events.
    deltas: bool,
    /// A turn's usage carries dollars.
    cost: bool,
    /// What the rate-limit fixture says about when the limit lifts.
    reset: Reset,
    /// Every tool call is answered by a tool result event. (`codex exec` reports an MCP or patch item once, on completion, as
    /// a call only; the app-server protocol has both halves.)
    tool_results: bool,
    /// Text that arrived before the process died is still reported as a message (Grok's one-shot stream flushes what it has).
    partial_on_crash: bool,
    /// Any of these appears in the spawn arguments, environment names or protocol requests when an MCP server is configured.
    mcp: &'static [&'static str],
    /// Same, for `Access::ReadOnly`.
    read_only: &'static [&'static str],
}

struct Subject {
    name: &'static str,
    kind: AgentKind,
    /// The harness over a spawner; `live` false switches the live protocol off.
    build: fn(Arc<dyn Spawner>, bool) -> Arc<dyn Harness>,
    /// The `tests/fake_cli` mode that speaks this CLI's long-lived protocol. `None`: live runs as spawn-per-turn.
    fifo: Option<&'static str>,
    one_shot: Fixtures,
    live: Fixtures,
    one_shot_expect: Expect,
    live_expect: Expect,
}

impl Subject {
    fn fixtures(&self, mode: Mode) -> &Fixtures {
        match mode {
            Mode::OneShot => &self.one_shot,
            Mode::Live => &self.live,
        }
    }

    fn expect(&self, mode: Mode) -> &Expect {
        match mode {
            Mode::OneShot => &self.one_shot_expect,
            Mode::Live => &self.live_expect,
        }
    }

    fn ctx(&self, mode: Mode, scenario: &str) -> String {
        format!("{} / {} / {scenario}", self.name, mode.name())
    }
}

fn cli(spawner: Arc<dyn Spawner>, live: bool, env: &[(&str, &str)]) -> CliAgentOptions {
    CliAgentOptions {
        spawner: Some(spawner),
        env: Some(env_of(env)),
        kill_grace: Some(Duration::from_millis(50)),
        disable_live_session: !live,
        ..CliAgentOptions::default()
    }
}

fn subjects() -> Vec<Subject> {
    let claude = Subject {
        name: "claude-code",
        kind: AgentKind::ClaudeCode,
        build: |spawner, live| {
            Arc::new(ClaudeCodeAdapter::new(ClaudeCodeOptions {
                cli: cli(spawner, live, &[("PATH", "/bin")]),
                allow_subscription_auth: true,
                ..ClaudeCodeOptions::default()
            }))
        },
        fifo: Some("claude-stream-json"),
        one_shot: Fixtures {
            start: "claude-2.1.316-mock-success.jsonl",
            resume: "claude-2.1.316-mock-resume.jsonl",
            deltas: "claude-2.1.316-mock-deltas.jsonl",
            usage: "claude-2.1.316-mock-success.jsonl",
            tool: "claude-2.1.316-mock-success.jsonl",
            mcp_tool: "claude-2.1.316-mock-mcp-tool.jsonl",
            read_only: "claude-2.1.316-mock-read-only-refusal.jsonl",
            rate_limit: "claude-2.1.316-mock-rate-limit.jsonl",
            auth_expired: "claude-2.1.316-mock-auth-expired.jsonl",
            crash: "claude-2.1.316-mock-crash.jsonl",
            interrupt: "claude-2.1.316-mock-deltas.jsonl",
        },
        live: Fixtures {
            start: "claude-2.1.316-mock-session.jsonl",
            resume: "claude-2.1.316-mock-session.jsonl",
            deltas: "claude-2.1.316-mock-session.jsonl",
            usage: "claude-2.1.316-mock-session.jsonl",
            tool: "claude-2.1.316-mock-mcp-tool.jsonl",
            mcp_tool: "claude-2.1.316-mock-mcp-tool.jsonl",
            read_only: "claude-2.1.316-mock-read-only-refusal.jsonl",
            rate_limit: "claude-2.1.316-mock-rate-limit.jsonl",
            auth_expired: "claude-2.1.316-mock-auth-expired.jsonl",
            crash: "claude-2.1.316-mock-crash.jsonl",
            interrupt: "claude-2.1.316-mock-interrupt.jsonl",
        },
        one_shot_expect: Expect {
            deltas: true,
            cost: true,
            reset: Reset::At(1793614200),
            tool_results: true,
            partial_on_crash: false,
            mcp: &["--mcp-config"],
            read_only: &["dontAsk"],
        },
        live_expect: Expect {
            deltas: true,
            cost: true,
            reset: Reset::At(1793614200),
            tool_results: true,
            partial_on_crash: false,
            mcp: &["--mcp-config"],
            read_only: &["dontAsk"],
        },
    };
    let codex = Subject {
        name: "codex",
        kind: AgentKind::Codex,
        build: |spawner, live| {
            Arc::new(CodexAdapter::new(CodexOptions {
                cli: cli(spawner, live, &[("PATH", "/bin")]),
                allow_subscription_auth: true,
                ..CodexOptions::default()
            }))
        },
        fifo: Some("codex-app-server"),
        one_shot: Fixtures {
            start: "codex-0.166.0-mock-success.jsonl",
            resume: "codex-0.166.0-mock-resume.jsonl",
            // `codex exec --json` sends each message whole; deltas are an app-server feature
            deltas: "codex-0.166.0-mock-success.jsonl",
            usage: "codex-0.166.0-mock-success.jsonl",
            tool: "codex-0.166.0-mock-success.jsonl",
            mcp_tool: "codex-0.166.0-mock-mcp-tool.jsonl",
            read_only: "codex-0.166.0-mock-read-only-refusal.jsonl",
            rate_limit: "codex-0.166.0-mock-rate-limit.jsonl",
            auth_expired: "codex-0.166.0-mock-auth-expired.jsonl",
            crash: "codex-0.166.0-mock-crash.jsonl",
            interrupt: "codex-0.166.0-mock-success.jsonl",
        },
        live: Fixtures {
            start: "codex-0.166.0-mock-session.jsonl",
            resume: "codex-0.166.0-mock-session.jsonl",
            deltas: "codex-0.166.0-mock-session.jsonl",
            usage: "codex-0.166.0-mock-session.jsonl",
            tool: "codex-0.166.0-mock-app-server-mcp-tool.jsonl",
            mcp_tool: "codex-0.166.0-mock-app-server-mcp-tool.jsonl",
            read_only: "codex-0.166.0-mock-app-server-read-only-refusal.jsonl",
            rate_limit: "codex-0.166.0-mock-app-server-rate-limit.jsonl",
            auth_expired: "codex-0.166.0-mock-app-server-auth-expired.jsonl",
            crash: "codex-0.166.0-mock-app-server-crash.jsonl",
            interrupt: "codex-0.166.0-mock-app-server-interrupt.jsonl",
        },
        one_shot_expect: Expect {
            deltas: false,
            cost: true,
            reset: Reset::Unknown,
            tool_results: false,
            partial_on_crash: false,
            mcp: &["mcp_servers.ratchet"],
            read_only: &["read-only"],
        },
        live_expect: Expect {
            deltas: true,
            cost: true,
            reset: Reset::At(1793614200),
            tool_results: true,
            partial_on_crash: false,
            mcp: &["ratchet"],
            read_only: &["read-only"],
        },
    };
    let grok = Subject {
        name: "grok",
        kind: AgentKind::Grok,
        build: |spawner, live| {
            Arc::new(GrokAdapter::new(GrokOptions {
                cli: cli(spawner, live, &[("PATH", "/bin"), ("XAI_API_KEY", "xai-k")]),
                ..GrokOptions::default()
            }))
        },
        fifo: Some("grok-acp"),
        one_shot: Fixtures {
            start: "grok-1.0.70-mock-success.jsonl",
            resume: "grok-1.0.70-mock-resume.jsonl",
            deltas: "grok-1.0.70-mock-deltas.jsonl",
            usage: "grok-1.0.70-mock-success.jsonl",
            tool: "grok-1.0.70-mock-success.jsonl",
            mcp_tool: "grok-1.0.70-mock-mcp-tool.jsonl",
            read_only: "grok-1.0.70-mock-read-only-refusal.jsonl",
            rate_limit: "grok-1.0.70-mock-rate-limit.jsonl",
            auth_expired: "grok-1.0.70-mock-auth-expired.jsonl",
            crash: "grok-1.0.70-mock-crash.jsonl",
            interrupt: "grok-1.0.70-mock-success.jsonl",
        },
        live: Fixtures {
            start: "grok-1.0.70-mock-session.jsonl",
            resume: "grok-1.0.70-mock-session.jsonl",
            deltas: "grok-1.0.70-mock-session.jsonl",
            usage: "grok-1.0.70-mock-acp-mcp-tool.jsonl",
            tool: "grok-1.0.70-mock-acp-mcp-tool.jsonl",
            mcp_tool: "grok-1.0.70-mock-acp-mcp-tool.jsonl",
            read_only: "grok-1.0.70-mock-acp-read-only-refusal.jsonl",
            rate_limit: "grok-1.0.70-mock-acp-rate-limit.jsonl",
            auth_expired: "grok-1.0.70-mock-acp-auth-expired.jsonl",
            crash: "grok-1.0.70-mock-acp-crash.jsonl",
            interrupt: "grok-1.0.70-mock-acp-interrupt.jsonl",
        },
        one_shot_expect: Expect {
            deltas: false,
            cost: true,
            reset: Reset::Soon,
            tool_results: true,
            partial_on_crash: true,
            mcp: &["GROK_HOME"],
            read_only: &["read-only"],
        },
        live_expect: Expect {
            deltas: true,
            cost: true,
            reset: Reset::Soon,
            tool_results: true,
            partial_on_crash: false,
            mcp: &["mcpServers"],
            read_only: &["--tools"],
        },
    };
    vec![claude, codex, grok, echo_subject()]
}

/// The fourth harness: a test-only CLI with its own adapter (`tests/conformance/echo.rs`) and fixtures. This row is all the
/// suite knows about it.
fn echo_subject() -> Subject {
    const F: Fixtures = Fixtures {
        start: "echo-0.1.0-mock-start.jsonl",
        resume: "echo-0.1.0-mock-resume.jsonl",
        deltas: "echo-0.1.0-mock-deltas.jsonl",
        usage: "echo-0.1.0-mock-start.jsonl",
        tool: "echo-0.1.0-mock-tool.jsonl",
        mcp_tool: "echo-0.1.0-mock-mcp-tool.jsonl",
        read_only: "echo-0.1.0-mock-read-only-refusal.jsonl",
        rate_limit: "echo-0.1.0-mock-rate-limit.jsonl",
        auth_expired: "echo-0.1.0-mock-auth-expired.jsonl",
        crash: "echo-0.1.0-mock-crash.jsonl",
        interrupt: "echo-0.1.0-mock-interrupt.jsonl",
    };
    const X: Expect = Expect {
        deltas: true,
        cost: true,
        reset: Reset::At(1793614200),
        tool_results: true,
        partial_on_crash: false,
        mcp: &["--mcp"],
        read_only: &["--read-only"],
    };
    Subject {
        name: "echo",
        kind: AgentKind::Mock,
        build: |spawner, _live| {
            Arc::new(echo::EchoHarness::new(CliAgentOptions {
                spawner: Some(spawner),
                env: Some(env_of(&[("PATH", "/bin")])),
                kill_grace: Some(Duration::from_millis(50)),
                ..CliAgentOptions::default()
            }))
        },
        fifo: None,
        one_shot: F,
        live: F,
        one_shot_expect: X,
        live_expect: X,
    }
}

// -------------------------------------------------------------------------------------------------------------- wiring

/// One step of the script a spawner plays: the n-th spawn plays the n-th step (the last repeats).
#[derive(Clone, Copy)]
enum Step {
    /// The whole fixture, then exit (with the code of a `{"$exit":N}` line, else 0).
    Replay(&'static str),
    /// [`Step::Replay`] for a CLI that reports a failure and exits non-zero (a `{"$exit":N}` line still wins).
    Fail(&'static str),
    /// The head of the fixture (up to `{"$hold":true}`, else two lines), then the process hangs until it is killed.
    Hang(&'static str),
}

impl Step {
    fn file(self) -> &'static str {
        match self {
            Step::Replay(f) | Step::Fail(f) | Step::Hang(f) => f,
        }
    }
}

fn process_for(step: Step) -> FakeProcessSpec {
    let text = fixture(step.file());
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    match step {
        Step::Replay(_) | Step::Fail(_) => {
            let mut code = i32::from(matches!(step, Step::Fail(_)));
            let mut out = String::new();
            for line in lines {
                match serde_json::from_str::<serde_json::Value>(line)
                    .ok()
                    .and_then(|v| v.get("$exit").and_then(|c| c.as_i64()))
                {
                    Some(c) => {
                        code = c as i32;
                        break;
                    }
                    None => {
                        out.push_str(line);
                        out.push('\n');
                    }
                }
            }
            FakeProcessSpec::stdout(out).with_exit(code)
        }
        Step::Hang(_) => {
            let cut = lines
                .iter()
                .position(|l| l.contains("\"$hold\""))
                .unwrap_or(lines.len().min(2));
            let mut out = lines[..cut].join("\n");
            out.push('\n');
            FakeProcessSpec::stdout(out).hanging()
        }
    }
}

/// The spawner behind a harness, and what the harness handed it.
struct Wire {
    spawner: Arc<dyn Spawner>,
    fake: Option<FakeSpawner>,
    fifo: Option<Redirect>,
    workdir: tempfile::TempDir,
}

impl Wire {
    fn new(subject: &Subject, mode: Mode, steps: &[Step]) -> Self {
        let workdir = tempfile::tempdir().unwrap();
        match (mode, subject.fifo) {
            (Mode::Live, Some(fifo)) => {
                let script: Vec<(&'static str, &'static str)> =
                    steps.iter().map(|s| (fifo, s.file())).collect();
                let redirect = Redirect::new(&script, workdir.path().join("stdin.log"));
                Wire {
                    spawner: Arc::new(redirect.clone()),
                    fake: None,
                    fifo: Some(redirect),
                    workdir,
                }
            }
            _ => {
                let steps = steps.to_vec();
                let fake =
                    FakeSpawner::with_plan(move |_, i| process_for(steps[i.min(steps.len() - 1)]));
                Wire {
                    spawner: Arc::new(fake.clone()),
                    fake: Some(fake),
                    fifo: None,
                    workdir,
                }
            }
        }
    }

    fn workdir(&self) -> String {
        self.workdir.path().to_string_lossy().into_owned()
    }

    fn spawns(&self) -> usize {
        match (&self.fake, &self.fifo) {
            (Some(f), _) => f.call_count(),
            (_, Some(r)) => r.spawns(),
            _ => 0,
        }
    }

    /// Everything spawn `n` was given that a test can read: arguments, environment names, and (live) what was written to the
    /// process, for the whole run.
    fn trace(&self, n: usize) -> String {
        if let Some(f) = &self.fake {
            let call = f.call(n);
            let mut env: Vec<&String> = call.options.env.keys().collect();
            env.sort();
            format!("{:?} {env:?} {}", call.args, call.stdin)
        } else if let Some(r) = &self.fifo {
            format!(
                "{:?} {}",
                r.spawn_args(n),
                r.requests()
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join("\n")
            )
        } else {
            String::new()
        }
    }

    fn signals(&self, n: usize) -> usize {
        self.fake.as_ref().map_or(0, |f| f.call(n).signals.len())
    }
}

fn ratchet_server() -> McpServer {
    McpServer {
        name: "ratchet".into(),
        command: "/usr/local/bin/ratchetd".into(),
        args: vec!["mcp".into()],
        env: [("TOKEN".to_string(), "t".to_string())].into(),
    }
}

fn agent_spec(subject_wire: &Wire, mcp: bool) -> AgentSpec {
    AgentSpec {
        workdir: subject_wire.workdir(),
        prompt: "hello".into(),
        mcp: if mcp { vec![ratchet_server()] } else { vec![] },
        ..AgentSpec::default()
    }
}

fn session_spec(wire: &Wire, access: Access, mcp: bool) -> SessionSpec {
    SessionSpec {
        workdir: wire.workdir(),
        access,
        mcp: if mcp { vec![ratchet_server()] } else { vec![] },
        ..SessionSpec::default()
    }
}

fn sink() -> (Events, SharedEventSink) {
    let (events, sink) = collect();
    (events, Arc::new(sink))
}

async fn within<T>(what: &str, f: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(20), f)
        .await
        .unwrap_or_else(|_| panic!("{what}: timed out"))
}

fn snapshot(events: &Events) -> Vec<AgentEvent> {
    events.lock().unwrap().clone()
}

fn of(events: &[AgentEvent], t: AgentEventType) -> Vec<&AgentEvent> {
    events.iter().filter(|e| e.event_type == t).collect()
}

fn joined(events: &[&AgentEvent]) -> String {
    events
        .iter()
        .map(|e| e.text.clone().unwrap_or_default())
        .collect()
}

fn failed(res: &PortResult<impl Sized>) -> bool {
    matches!(res, Err(PortError::AgentFailed { .. }))
}

/// A live session opened for `mode`'s harness, plus its events.
struct Live {
    session: Box<dyn LiveSession>,
    events: Events,
}

async fn open(subject: &Subject, mode: Mode, wire: &Wire, spec: &SessionSpec, what: &str) -> Live {
    let harness = (subject.build)(wire.spawner.clone(), mode == Mode::Live);
    let (events, sink) = sink();
    let session = within(what, harness.open(spec, sink))
        .await
        .unwrap_or_else(|e| panic!("{what}: open failed: {e}"));
    Live { session, events }
}

fn harness(subject: &Subject, mode: Mode, wire: &Wire) -> Arc<dyn Harness> {
    (subject.build)(wire.spawner.clone(), mode == Mode::Live)
}

fn mentions(trace: &str, any: &[&str]) -> bool {
    any.iter().any(|m| trace.contains(m))
}

// ------------------------------------------------------------------------------------------------------------ scenarios

/// A normal turn produces a message; the session has an id.
async fn start(s: &Subject, mode: Mode) {
    let ctx = s.ctx(mode, "start");
    let fx = s.fixtures(mode);
    match mode {
        Mode::OneShot => {
            let wire = Wire::new(s, mode, &[Step::Replay(fx.start)]);
            let (events, sink) = collect();
            let session = within(
                &ctx,
                harness(s, mode, &wire).start(&agent_spec(&wire, false), &sink, None),
            )
            .await
            .unwrap_or_else(|e| panic!("{ctx}: {e}"));
            let events = snapshot(&events);
            assert!(!session.id.is_empty(), "{ctx}: session id");
            assert_eq!(session.kind, s.kind, "{ctx}");
            let messages = of(&events, AgentEventType::Message);
            assert!(!messages.is_empty(), "{ctx}: a message");
            assert!(
                messages
                    .iter()
                    .all(|m| !m.text.clone().unwrap_or_default().is_empty()),
                "{ctx}: messages have text"
            );
            assert!(
                of(&events, AgentEventType::Error).is_empty(),
                "{ctx}: no error"
            );
        }
        Mode::Live => {
            let wire = Wire::new(s, mode, &[Step::Replay(fx.start), Step::Replay(fx.resume)]);
            let live = open(
                s,
                mode,
                &wire,
                &session_spec(&wire, Access::ReadOnly, false),
                &ctx,
            )
            .await;
            let early = snapshot(&live.events);
            assert!(
                early.iter().all(|e| e.event_type == AgentEventType::Status),
                "{ctx}: nothing but status before the first message: {early:?}"
            );
            within(&ctx, live.session.send("hello"))
                .await
                .unwrap_or_else(|e| panic!("{ctx}: {e}"));
            assert!(
                live.session.session_id().is_some_and(|i| !i.is_empty()),
                "{ctx}: session id"
            );
            within(&ctx, live.session.send("and now?"))
                .await
                .unwrap_or_else(|e| panic!("{ctx}: {e}"));
            let events = snapshot(&live.events);
            let messages = of(&events, AgentEventType::Message);
            assert_eq!(messages.len(), 2, "{ctx}: one message per turn");
            assert!(
                messages
                    .iter()
                    .all(|m| m.text.as_deref().is_some_and(|t| !t.is_empty())),
                "{ctx}"
            );
            assert!(
                of(&events, AgentEventType::Error).is_empty(),
                "{ctx}: no error"
            );
            // a live protocol keeps one process for both turns; spawn-per-turn starts one each
            let expected = if s.fifo.is_some() { 1 } else { 2 };
            assert_eq!(wire.spawns(), expected, "{ctx}: processes for two turns");
            within(&ctx, live.session.close()).await;
        }
    }
}

/// The next turn continues the same CLI session.
async fn resume(s: &Subject, mode: Mode) {
    let ctx = s.ctx(mode, "resume");
    let fx = s.fixtures(mode);
    let wire = Wire::new(s, mode, &[Step::Replay(fx.start), Step::Replay(fx.resume)]);
    match mode {
        Mode::OneShot => {
            let h = harness(s, mode, &wire);
            let spec = agent_spec(&wire, false);
            let (events, sink) = collect();
            let mut session = within(&ctx, h.start(&spec, &sink, None))
                .await
                .unwrap_or_else(|e| panic!("{ctx}: {e}"));
            let first = session.id.clone();
            within(&ctx, h.resume(&mut session, &spec, "next", &sink, None))
                .await
                .unwrap_or_else(|e| panic!("{ctx}: {e}"));
            assert_eq!(wire.spawns(), 2, "{ctx}");
            assert!(
                wire.trace(1).contains(&first),
                "{ctx}: the second turn names session {first}: {}",
                wire.trace(1)
            );
            assert!(!session.id.is_empty(), "{ctx}");
            let events = snapshot(&events);
            assert!(
                of(&events, AgentEventType::Message).len() >= 2,
                "{ctx}: a message per turn"
            );
        }
        Mode::Live => {
            let spec = session_spec(&wire, Access::ReadOnly, false);
            let first = open(s, mode, &wire, &spec, &ctx).await;
            within(&ctx, first.session.send("hello"))
                .await
                .unwrap_or_else(|e| panic!("{ctx}: {e}"));
            let id = first
                .session
                .session_id()
                .unwrap_or_else(|| panic!("{ctx}: no session id"));
            within(&ctx, first.session.close()).await;
            let spawns = wire.spawns();

            let mut spec = spec;
            spec.resume = Some(id.clone());
            let second = open(s, mode, &wire, &spec, &ctx).await;
            assert_eq!(
                second.session.session_id().as_deref(),
                Some(id.as_str()),
                "{ctx}: the id is known before the first message"
            );
            within(&ctx, second.session.send("welcome back"))
                .await
                .unwrap_or_else(|e| panic!("{ctx}: {e}"));
            assert_eq!(
                wire.spawns(),
                spawns + 1,
                "{ctx}: the reopened session is one new process"
            );
            assert!(
                wire.trace(spawns).contains(&id),
                "{ctx}: the reopened session names {id}"
            );
            assert!(
                !of(&snapshot(&second.events), AgentEventType::Message).is_empty(),
                "{ctx}"
            );
            within(&ctx, second.session.close()).await;
        }
    }
}

/// Partial text comes before the whole message and adds up to it, or does not come at all.
async fn deltas(s: &Subject, mode: Mode) {
    let ctx = s.ctx(mode, "deltas");
    let fx = s.fixtures(mode);
    let wire = Wire::new(s, mode, &[Step::Replay(fx.deltas)]);
    let events = match mode {
        Mode::OneShot => {
            let (events, sink) = collect();
            within(
                &ctx,
                harness(s, mode, &wire).start(&agent_spec(&wire, false), &sink, None),
            )
            .await
            .unwrap_or_else(|e| panic!("{ctx}: {e}"));
            snapshot(&events)
        }
        Mode::Live => {
            let live = open(
                s,
                mode,
                &wire,
                &session_spec(&wire, Access::ReadOnly, false),
                &ctx,
            )
            .await;
            within(&ctx, live.session.send("hello"))
                .await
                .unwrap_or_else(|e| panic!("{ctx}: {e}"));
            let events = snapshot(&live.events);
            within(&ctx, live.session.close()).await;
            events
        }
    };
    let deltas = of(&events, AgentEventType::Delta);
    let messages = of(&events, AgentEventType::Message);
    assert!(
        !messages.is_empty(),
        "{ctx}: the whole message still arrives"
    );
    if s.expect(mode).deltas {
        assert!(deltas.len() >= 2, "{ctx}: {} deltas", deltas.len());
        let last_delta = events
            .iter()
            .rposition(|e| e.event_type == AgentEventType::Delta)
            .unwrap();
        let first_message = events
            .iter()
            .position(|e| e.event_type == AgentEventType::Message)
            .unwrap();
        assert!(
            last_delta < first_message,
            "{ctx}: deltas come before the whole message"
        );
        assert_eq!(
            joined(&deltas),
            joined(&messages[..1]),
            "{ctx}: deltas add up to the message"
        );
    } else {
        assert!(deltas.is_empty(), "{ctx}: this CLI sends messages whole");
    }
}

/// A tool call and its result are paired events.
async fn tool_call_and_result(s: &Subject, mode: Mode) {
    let ctx = s.ctx(mode, "tool call and result");
    let fx = s.fixtures(mode);
    let wire = Wire::new(s, mode, &[Step::Replay(fx.tool)]);
    let events = run_one_turn(s, mode, &wire, &ctx, false).await.1;
    assert_tool_pairs(&events, &ctx, s.expect(mode).tool_results);
}

/// Every tool result has a call before it; every call is answered.
fn assert_tool_pairs(events: &[AgentEvent], ctx: &str, results: bool) {
    let tools = of(events, AgentEventType::Tool);
    let phase = |e: &AgentEvent| {
        e.data
            .as_ref()
            .and_then(|d| d["phase"].as_str())
            .unwrap_or("")
            .to_string()
    };
    let calls = tools.iter().filter(|e| phase(e) == "call").count();
    let paired = results;
    let results = tools.iter().filter(|e| phase(e) == "result").count();
    assert!(calls >= 1, "{ctx}: a tool call");
    if !paired {
        assert!(calls >= 1, "{ctx}: a tool call");
        return;
    }
    assert_eq!(calls, results, "{ctx}: every call has a result");
    let mut open_calls = 0i32;
    for t in &tools {
        match phase(t).as_str() {
            "call" => open_calls += 1,
            "result" => {
                open_calls -= 1;
                assert!(open_calls >= 0, "{ctx}: a result before its call");
                assert!(
                    t.data.as_ref().is_some_and(|d| d["isError"].is_boolean()),
                    "{ctx}: results say whether they failed"
                );
            }
            other => panic!("{ctx}: unknown tool phase {other:?}"),
        }
    }
}

/// One successful turn of `fx`'s fixture; returns the spawn trace and the events.
async fn run_one_turn(
    s: &Subject,
    mode: Mode,
    wire: &Wire,
    ctx: &str,
    mcp: bool,
) -> (String, Vec<AgentEvent>) {
    match mode {
        Mode::OneShot => {
            let (events, sink) = collect();
            within(
                ctx,
                harness(s, mode, wire).start(&agent_spec(wire, mcp), &sink, None),
            )
            .await
            .unwrap_or_else(|e| panic!("{ctx}: {e}"));
            (wire.trace(0), snapshot(&events))
        }
        Mode::Live => {
            let live = open(
                s,
                mode,
                wire,
                &session_spec(wire, Access::ReadOnly, mcp),
                ctx,
            )
            .await;
            within(ctx, live.session.send("hello"))
                .await
                .unwrap_or_else(|e| panic!("{ctx}: {e}"));
            let events = snapshot(&live.events);
            let trace = wire.trace(0);
            within(ctx, live.session.close()).await;
            (trace, events)
        }
    }
}

/// A turn reports its tokens and, where the CLI knows it, its dollars.
async fn usage_and_cost(s: &Subject, mode: Mode) {
    let ctx = s.ctx(mode, "usage and cost");
    let wire = Wire::new(s, mode, &[Step::Replay(s.fixtures(mode).usage)]);
    let events = run_one_turn(s, mode, &wire, &ctx, false).await.1;
    let usage: Vec<_> = events.iter().filter_map(|e| e.usage.as_ref()).collect();
    assert!(!usage.is_empty(), "{ctx}: usage");
    assert!(
        usage.iter().any(|u| u.input_tokens + u.output_tokens > 0),
        "{ctx}: tokens"
    );
    let priced = usage.iter().any(|u| u.cost_usd.is_some_and(|c| c > 0.0));
    assert_eq!(priced, s.expect(mode).cost, "{ctx}: dollars reported");
    assert!(
        events
            .iter()
            .filter(|e| e.event_type == AgentEventType::Usage)
            .all(|e| e.usage.is_some()),
        "{ctx}: usage events carry usage"
    );
}

/// What a failing turn leaves behind, one-shot or live.
struct Failure {
    result: PortResult<()>,
    events: Vec<AgentEvent>,
}

async fn fail_one_turn(
    s: &Subject,
    mode: Mode,
    fixture: &'static str,
    ctx: &str,
) -> (Failure, Wire) {
    let wire = Wire::new(s, mode, &[Step::Fail(fixture)]);
    match mode {
        Mode::OneShot => {
            let (events, sink) = collect();
            let result = within(
                ctx,
                harness(s, mode, &wire).start(&agent_spec(&wire, false), &sink, None),
            )
            .await
            .map(|_| ());
            let events = snapshot(&events);
            (Failure { result, events }, wire)
        }
        Mode::Live => {
            let live = open(
                s,
                mode,
                &wire,
                &session_spec(&wire, Access::ReadOnly, false),
                ctx,
            )
            .await;
            let result = within(ctx, live.session.send("hello")).await;
            let events = snapshot(&live.events);
            within(ctx, live.session.close()).await;
            (Failure { result, events }, wire)
        }
    }
}

fn reset_of(e: &AgentEvent) -> Option<i64> {
    e.data.as_ref().and_then(|d| d["resetsAt"].as_i64())
}

/// Out of quota: one `rate_limited` event, the turn fails, and the reset time (seconds) is there exactly when the CLI gave it.
/// That time is what the fallback layer waits for; without it the layer switches harness or cools down.
async fn rate_limit(s: &Subject, mode: Mode) {
    let ctx = s.ctx(mode, "rate limit");
    let (f, _) = fail_one_turn(s, mode, s.fixtures(mode).rate_limit, &ctx).await;
    assert!(failed(&f.result), "{ctx}: {:?}", f.result);
    let limited = of(&f.events, AgentEventType::RateLimited);
    assert_eq!(limited.len(), 1, "{ctx}: exactly one rate_limited event");
    assert!(
        of(&f.events, AgentEventType::AuthExpired).is_empty(),
        "{ctx}: not an auth failure"
    );
    let reset = reset_of(limited[0]);
    match s.expect(mode).reset {
        Reset::Unknown => assert_eq!(
            reset, None,
            "{ctx}: the CLI did not say when the limit lifts"
        ),
        Reset::At(at) => assert_eq!(reset, Some(at), "{ctx}: the reset time, epoch seconds"),
        Reset::Soon => {
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_secs() as i64;
            let at = reset.unwrap_or_else(|| panic!("{ctx}: a relative reset time"));
            assert!(
                (now..now + 3600).contains(&at),
                "{ctx}: {at} is not within the next hour (epoch seconds)"
            );
        }
    }
}

/// A rejected login: one `auth_expired` event, no reset time, the turn fails. The fallback layer always switches on this.
async fn auth_expired(s: &Subject, mode: Mode) {
    let ctx = s.ctx(mode, "auth expired");
    let (f, _) = fail_one_turn(s, mode, s.fixtures(mode).auth_expired, &ctx).await;
    assert!(failed(&f.result), "{ctx}: {:?}", f.result);
    let expired = of(&f.events, AgentEventType::AuthExpired);
    assert_eq!(expired.len(), 1, "{ctx}: exactly one auth_expired event");
    assert!(
        of(&f.events, AgentEventType::RateLimited).is_empty(),
        "{ctx}: not a rate limit"
    );
    assert!(
        reset_of(expired[0]).is_none(),
        "{ctx}: no reset time to wait for"
    );
}

/// The process dies mid-turn: an `error` event and a failed turn, never a message and never a classified failure.
async fn crash(s: &Subject, mode: Mode) {
    let ctx = s.ctx(mode, "crash");
    let (f, _) = fail_one_turn(s, mode, s.fixtures(mode).crash, &ctx).await;
    assert!(failed(&f.result), "{ctx}: {:?}", f.result);
    assert!(
        !of(&f.events, AgentEventType::Error).is_empty(),
        "{ctx}: an error event"
    );
    if !s.expect(mode).partial_on_crash {
        assert!(
            of(&f.events, AgentEventType::Message).is_empty(),
            "{ctx}: half a turn is not a message"
        );
    }
    assert!(
        of(&f.events, AgentEventType::RateLimited).is_empty(),
        "{ctx}"
    );
    assert!(
        of(&f.events, AgentEventType::AuthExpired).is_empty(),
        "{ctx}"
    );
}

/// Stopping a turn is not a failure; the session (live) carries on.
async fn interrupt(s: &Subject, mode: Mode) {
    let ctx = s.ctx(mode, "interrupt");
    let fx = s.fixtures(mode);
    match mode {
        Mode::OneShot => {
            let wire = Wire::new(s, mode, &[Step::Hang(fx.interrupt)]);
            let h = harness(s, mode, &wire);
            let (events, sink) = collect();
            let cancel = CancellationToken::new();
            let spec = agent_spec(&wire, false);
            let (res, ()) = within(&ctx, async {
                tokio::join!(h.start(&spec, &sink, Some(&cancel)), async {
                    while wire.spawns() == 0 {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    cancel.cancel();
                })
            })
            .await;
            assert!(matches!(res, Err(PortError::Cancelled)), "{ctx}: {res:?}");
            assert!(wire.signals(0) > 0, "{ctx}: the process was told to stop");
            assert!(
                of(&snapshot(&events), AgentEventType::Error).is_empty(),
                "{ctx}: not an error"
            );
        }
        Mode::Live => {
            // a spawn-per-turn session stops the process of the turn; the next turn is a fresh one
            let steps = if s.fifo.is_some() {
                vec![Step::Replay(fx.interrupt)]
            } else {
                vec![Step::Hang(fx.interrupt), Step::Replay(fx.resume)]
            };
            let wire = Wire::new(s, mode, &steps);
            let live = open(
                s,
                mode,
                &wire,
                &session_spec(&wire, Access::ReadOnly, false),
                &ctx,
            )
            .await;
            let (sent, ()) = within(&ctx, async {
                tokio::join!(live.session.send("count the runs"), async {
                    let started = std::time::Instant::now();
                    while of(&snapshot(&live.events), AgentEventType::Delta).is_empty() {
                        assert!(
                            started.elapsed() < Duration::from_secs(10),
                            "{ctx}: no delta to interrupt"
                        );
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    live.session.interrupt().await.unwrap();
                })
            })
            .await;
            sent.unwrap_or_else(|e| panic!("{ctx}: an interrupted turn ends Ok: {e}"));
            assert!(
                of(&snapshot(&live.events), AgentEventType::Error).is_empty(),
                "{ctx}: not an error"
            );
            let before = of(&snapshot(&live.events), AgentEventType::Message).len();
            within(&ctx, live.session.send("go on"))
                .await
                .unwrap_or_else(|e| panic!("{ctx}: after: {e}"));
            assert!(
                of(&snapshot(&live.events), AgentEventType::Message).len() > before,
                "{ctx}: the session survived the interrupt"
            );
            // interrupting while idle does nothing
            live.session.interrupt().await.unwrap();
            within(&ctx, live.session.close()).await;
        }
    }
}

/// The Ratchet MCP server reaches the CLI, and its tool calls come back as tool events.
async fn mcp_tool_call(s: &Subject, mode: Mode) {
    let ctx = s.ctx(mode, "mcp tool call");
    let wire = Wire::new(s, mode, &[Step::Replay(s.fixtures(mode).mcp_tool)]);
    let (trace, events) = run_one_turn(s, mode, &wire, &ctx, true).await;
    assert!(
        mentions(&trace, s.expect(mode).mcp),
        "{ctx}: the server is handed to the CLI: {trace}"
    );
    assert_tool_pairs(&events, &ctx, s.expect(mode).tool_results);
    let call = of(&events, AgentEventType::Tool)
        .into_iter()
        .find(|e| {
            e.data.as_ref().is_some_and(|d| {
                d["phase"] == "call"
                    && d["name"].as_str().is_none_or(|n| n.contains("ratchet"))
                    && e.text.as_deref().unwrap_or("").contains("ratchet")
            })
        })
        .unwrap_or_else(|| panic!("{ctx}: a call to the ratchet server: {events:?}"));
    assert!(
        call.text.as_deref().unwrap().contains("list_optimizations"),
        "{ctx}"
    );
    if s.expect(mode).tool_results {
        let first_result = of(&events, AgentEventType::Tool)
            .into_iter()
            .find(|e| e.data.as_ref().is_some_and(|d| d["phase"] == "result"))
            .unwrap();
        assert_eq!(
            first_result.data.as_ref().unwrap()["isError"],
            false,
            "{ctx}: the MCP call worked"
        );
    }
    assert!(!of(&events, AgentEventType::Message).is_empty(), "{ctx}");
}

/// A read-only session says so to the CLI, and a refused write is an errored tool result, not a failed turn.
async fn read_only_refusal(s: &Subject, mode: Mode) {
    let ctx = s.ctx(mode, "read-only refusal");
    let fx = s.fixtures(mode);
    // the refused write, through the way this mode drives the harness
    let wire = Wire::new(s, mode, &[Step::Replay(fx.read_only)]);
    let events = run_one_turn(s, mode, &wire, &ctx, false).await.1;
    let tools = of(&events, AgentEventType::Tool);
    assert!(
        tools.iter().any(|t| t
            .data
            .as_ref()
            .is_some_and(|d| d["phase"] == "result" && d["isError"] == true)),
        "{ctx}: the refused write is an errored tool result: {tools:?}"
    );
    assert!(
        of(&events, AgentEventType::Error).is_empty(),
        "{ctx}: the turn itself succeeded"
    );
    assert!(
        !of(&events, AgentEventType::Message).is_empty(),
        "{ctx}: the model explains"
    );

    // and the request itself is read-only
    let wire = Wire::new(s, mode, &[Step::Replay(fx.start), Step::Replay(fx.start)]);
    let live = open(
        s,
        mode,
        &wire,
        &session_spec(&wire, Access::ReadOnly, false),
        &ctx,
    )
    .await;
    within(&ctx, live.session.send("hello"))
        .await
        .unwrap_or_else(|e| panic!("{ctx}: {e}"));
    let trace = wire.trace(0);
    assert!(
        mentions(&trace, s.expect(mode).read_only),
        "{ctx}: read-only reaches the CLI: {trace}"
    );
    within(&ctx, live.session.close()).await;

    // an edit session does not carry it
    let wire = Wire::new(s, mode, &[Step::Replay(fx.start)]);
    let live = open(
        s,
        mode,
        &wire,
        &session_spec(&wire, Access::Edit { network: true }, false),
        &ctx,
    )
    .await;
    within(&ctx, live.session.send("hello"))
        .await
        .unwrap_or_else(|e| panic!("{ctx}: {e}"));
    let trace = wire.trace(0);
    assert!(
        !mentions(&trace, s.expect(mode).read_only),
        "{ctx}: an edit session is not read-only: {trace}"
    );
    within(&ctx, live.session.close()).await;
}

// ------------------------------------------------------------------------------------------------------- the table

/// A scenario run against one subject in one mode.
type Scenario = for<'a> fn(&'a Subject, Mode) -> std::pin::Pin<Box<dyn Future<Output = ()> + 'a>>;

macro_rules! scenario_table {
    ($( $test:ident => $f:ident ),* $(,)?) => {
        const SCENARIO_NAMES: &[&str] = &[$(stringify!($f)),*];

        $(
            #[tokio::test]
            async fn $test() {
                for subject in subjects() {
                    for mode in [Mode::OneShot, Mode::Live] {
                        $f(&subject, mode).await;
                    }
                }
            }
        )*
    };
}

scenario_table! {
    every_harness_starts_a_session => start,
    every_harness_resumes_a_session => resume,
    every_harness_streams_deltas_or_says_it_cannot => deltas,
    every_harness_reports_tool_calls_and_results => tool_call_and_result,
    every_harness_reports_usage_and_cost => usage_and_cost,
    every_harness_classifies_a_rate_limit_with_its_reset_time => rate_limit,
    every_harness_classifies_an_expired_login => auth_expired,
    every_harness_survives_a_crash_as_a_failed_turn => crash,
    every_harness_interrupts_without_failing => interrupt,
    every_harness_passes_mcp_tool_calls_through => mcp_tool_call,
    every_harness_refuses_writes_when_read_only => read_only_refusal,
}

#[test]
fn the_table_covers_every_scenario_the_plan_names() {
    for wanted in [
        "start",
        "resume",
        "deltas",
        "tool_call_and_result",
        "usage_and_cost",
        "rate_limit",
        "auth_expired",
        "crash",
        "interrupt",
        "mcp_tool_call",
        "read_only_refusal",
    ] {
        assert!(SCENARIO_NAMES.contains(&wanted), "no scenario {wanted}");
    }
}

#[test]
fn the_table_names_only_fixtures_that_exist_and_every_harness_has_all_of_them() {
    let dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for s in subjects() {
        for mode in [Mode::OneShot, Mode::Live] {
            let f = s.fixtures(mode);
            for name in [
                f.start,
                f.resume,
                f.deltas,
                f.usage,
                f.tool,
                f.mcp_tool,
                f.read_only,
                f.rate_limit,
                f.auth_expired,
                f.crash,
                f.interrupt,
            ] {
                assert!(
                    dir.join(name).is_file(),
                    "{} / {}: missing fixture {name}",
                    s.name,
                    mode.name()
                );
            }
        }
    }
}

/// The harness layer needs nothing from the suite but a `Subject` row: run only the fourth harness through every scenario.
#[tokio::test]
async fn a_fourth_harness_passes_the_whole_suite_with_nothing_but_an_adapter_and_fixtures() {
    let echo = echo_subject();
    let all: Vec<(&str, Scenario)> = vec![
        ("start", |s, m| Box::pin(start(s, m))),
        ("resume", |s, m| Box::pin(resume(s, m))),
        ("deltas", |s, m| Box::pin(deltas(s, m))),
        ("tool", |s, m| Box::pin(tool_call_and_result(s, m))),
        ("usage", |s, m| Box::pin(usage_and_cost(s, m))),
        ("rate limit", |s, m| Box::pin(rate_limit(s, m))),
        ("auth", |s, m| Box::pin(auth_expired(s, m))),
        ("crash", |s, m| Box::pin(crash(s, m))),
        ("interrupt", |s, m| Box::pin(interrupt(s, m))),
        ("mcp", |s, m| Box::pin(mcp_tool_call(s, m))),
        ("read-only", |s, m| Box::pin(read_only_refusal(s, m))),
    ];
    assert_eq!(all.len(), SCENARIO_NAMES.len(), "this list is the table");
    for (_, scenario) in &all {
        for mode in [Mode::OneShot, Mode::Live] {
            scenario(&echo, mode).await;
        }
    }
    // it is a harness like the others to everything above the adapter
    let wire = Wire::new(&echo, Mode::Live, &[Step::Replay(echo.live.start)]);
    let h = harness(&echo, Mode::Live, &wire);
    assert!(
        !h.capabilities().live_session,
        "echo has no live protocol: its chat is spawn-per-turn"
    );
    assert!(h.capabilities().mcp && h.capabilities().read_only);
    assert_eq!(h.kind(), AgentKind::Mock);
    let _ = json!({});
}
