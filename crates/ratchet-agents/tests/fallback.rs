//! P6: the runner loop's harness fallback (docs/AGENT_HARNESS_PLAN.md 4.6), steering read receipts, and the optimization tool set
//! on every turn. The harnesses are scripted: each turn does what the test queued (works, hits a rate limit, loses its sign-in,
//! crashes), and the git side is real.

mod common;

use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use common::{AutoOptions, Sandbox, auto_verdicts, base_config, make_sandbox, until};
use ratchet_agents::{
    AvailabilityProbe, FallbackPlan, HarnessSlot, LoopTuning, ManualClock, MemoryInbox,
    RunnerLoopOptions, RunnerLoopResult, RunnerStopReason, run_runner_loop,
};
use ratchet_core::AgentsPolicy;
use ratchet_core::enums::{AgentKind, CandidateVerdict};
use ratchet_core::ports::mock::SystemClock;
use ratchet_core::ports::{
    AgentAdapter, AgentEvent, AgentEventType, AgentSession, AgentSpec, CancellationToken, Clock,
    EventSink, McpServer, PortError, PortResult, Preflight, RunnerMessage,
};
use serde_json::{Value, json};

/// What one turn of a scripted harness does.
#[derive(Clone)]
enum Turn {
    /// Edit a file, so there is a candidate.
    Works,
    /// The provider refuses; `resets_at_ms` is when it lifts, if the CLI says.
    RateLimited {
        resets_at_ms: Option<i64>,
    },
    AuthExpired,
    Crash,
}

struct Harness {
    kind: AgentKind,
    turns: Mutex<VecDeque<Turn>>,
    /// The prompt or resume message of every call, with whether it was a start.
    calls: Mutex<Vec<(bool, String)>>,
    specs: Mutex<Vec<AgentSpec>>,
    signed_in: AtomicBool,
    counter: AtomicU32,
}

impl Harness {
    fn new(kind: AgentKind, turns: Vec<Turn>) -> Arc<Self> {
        Arc::new(Self {
            kind,
            turns: Mutex::new(turns.into()),
            calls: Mutex::new(Vec::new()),
            specs: Mutex::new(Vec::new()),
            signed_in: AtomicBool::new(true),
            counter: AtomicU32::new(0),
        })
    }
    fn calls(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
    fn starts(&self) -> usize {
        self.calls.lock().unwrap().iter().filter(|c| c.0).count()
    }
    fn message(&self, n: usize) -> String {
        self.calls.lock().unwrap()[n].1.clone()
    }

    fn run_turn(&self, spec: &AgentSpec, events: &EventSink<'_>) -> PortResult<()> {
        let turn = self
            .turns
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Turn::Works);
        let n = self.counter.fetch_add(1, Ordering::SeqCst);
        match turn {
            Turn::Works => {
                std::fs::write(
                    PathBuf::from(&spec.workdir).join(format!("{}-{n}.txt", self.kind)),
                    format!("{n}\n"),
                )
                .unwrap();
                Ok(())
            }
            Turn::RateLimited { resets_at_ms } => {
                events(
                    AgentEvent::new(0, AgentEventType::RateLimited)
                        .with_text("You've hit your usage limit")
                        .with_data(json!({"resetsAt": resets_at_ms.map(|ms| ms / 1000)})),
                );
                Err(PortError::Failed("usage limit reached".into()))
            }
            Turn::AuthExpired => {
                events(
                    AgentEvent::new(0, AgentEventType::AuthExpired).with_text("401 Unauthorized"),
                );
                Err(PortError::Failed("401 Unauthorized".into()))
            }
            Turn::Crash => Err(PortError::Failed("the stream ended in the middle".into())),
        }
    }
}

#[async_trait]
impl AgentAdapter for Harness {
    fn kind(&self) -> AgentKind {
        self.kind
    }
    async fn preflight(&self) -> PortResult<Preflight> {
        let ok = self.signed_in.load(Ordering::SeqCst);
        Ok(Preflight {
            ok,
            problems: if ok {
                vec![]
            } else {
                vec![format!("{} is signed out", self.kind)]
            },
        })
    }
    async fn start(
        &self,
        spec: &AgentSpec,
        events: &EventSink<'_>,
        _cancel: Option<&CancellationToken>,
    ) -> PortResult<AgentSession> {
        self.calls.lock().unwrap().push((true, spec.prompt.clone()));
        self.specs.lock().unwrap().push(spec.clone());
        self.run_turn(spec, events)?;
        Ok(AgentSession {
            id: format!("{}-session", self.kind),
            kind: self.kind,
        })
    }
    async fn resume(
        &self,
        _session: &mut AgentSession,
        spec: &AgentSpec,
        message: &str,
        events: &EventSink<'_>,
        _cancel: Option<&CancellationToken>,
    ) -> PortResult<()> {
        self.calls.lock().unwrap().push((false, message.to_owned()));
        self.specs.lock().unwrap().push(spec.clone());
        self.run_turn(spec, events)
    }
}

/// Detection, as the local daemon answers it.
struct Detection(Mutex<Vec<AgentKind>>);

#[async_trait]
impl AvailabilityProbe for Detection {
    async fn check(&self, kind: AgentKind) -> Result<(), String> {
        if self.0.lock().unwrap().contains(&kind) {
            Ok(())
        } else {
            Err(format!("{kind} is signed out"))
        }
    }
}

struct Rig {
    _sb: Arc<Sandbox>,
    inbox: Arc<MemoryInbox>,
    events: Arc<Mutex<Vec<AgentEvent>>>,
    task: Option<tokio::task::JoinHandle<RunnerLoopResult>>,
    workdir: PathBuf,
}

#[derive(Default)]
struct Opts {
    clock: Option<Arc<dyn Clock>>,
    probe: Option<Arc<dyn AvailabilityProbe>>,
    mcp: Vec<McpServer>,
    /// Answer every commit with a verdict (default true).
    manual: bool,
    policy: Option<AgentsPolicy>,
}

impl Rig {
    fn events(&self) -> Vec<AgentEvent> {
        self.events.lock().unwrap().clone()
    }
    fn harness_events(&self, kind: &str) -> Vec<AgentEvent> {
        self.events()
            .into_iter()
            .filter(|e| {
                e.event_type == AgentEventType::Status
                    && e.data.as_ref().and_then(|d| d["harness"]["event"].as_str()) == Some(kind)
            })
            .collect()
    }
    fn steering(&self, kind: &str) -> Vec<AgentEvent> {
        self.events()
            .into_iter()
            .filter(|e| {
                e.data
                    .as_ref()
                    .and_then(|d| d["steering"]["event"].as_str())
                    == Some(kind)
            })
            .collect()
    }
    fn has_text(&self, needle: &str) -> bool {
        self.events()
            .iter()
            .any(|e| e.text.as_deref().is_some_and(|t| t.contains(needle)))
    }
    async fn stop(&mut self) -> RunnerLoopResult {
        self.inbox.push(RunnerMessage::Stop);
        self.task.take().unwrap().await.unwrap()
    }
}

async fn launch(chain: &[Arc<Harness>], opts: Opts) -> Rig {
    let sb = Arc::new(make_sandbox());
    let clock: Arc<dyn Clock> = opts.clock.unwrap_or_else(|| Arc::new(SystemClock));
    let deadline = clock.now() + 24 * 3_600_000;
    let config = base_config(&sb, |c| {
        c["deadlineAt"] = json!(deadline);
        c["kind"] = json!(chain[0].kind.as_str());
    });
    let workdir = sb.dir.join("work-fallback");
    ratchet_agents::workdir::prepare_workdir(ratchet_agents::workdir::PrepareWorkdirOptions {
        repo_url: config.repo_url.clone(),
        branch: config.branch.clone(),
        base_ref: config.base_ref.clone(),
        dir: workdir.clone(),
        git: None,
        env: None,
    })
    .await
    .unwrap();
    let inbox = Arc::new(MemoryInbox::new());
    let events: Arc<Mutex<Vec<AgentEvent>>> = Arc::default();
    let answer = (!opts.manual).then(|| {
        let push_to = inbox.clone();
        auto_verdicts(move |m| push_to.push(m), AutoOptions::default())
    });
    let sink = events.clone();
    let policy = opts
        .policy
        .unwrap_or_else(|| AgentsPolicy::new(chain.iter().map(|h| h.kind)));
    let slots: Vec<HarnessSlot> = chain
        .iter()
        .map(|h| HarnessSlot::new(h.kind, h.clone() as Arc<dyn AgentAdapter>))
        .collect();
    let tuning = LoopTuning {
        max_idle_turns: Some(50),
        retry_delay: Some(Duration::from_millis(1)),
        fallback: Some(FallbackPlan {
            chain: slots,
            policy,
            probe: opts.probe,
        }),
        mcp: opts.mcp,
        ..LoopTuning::default()
    };
    let task = tokio::spawn(run_runner_loop(RunnerLoopOptions {
        config,
        adapter: chain[0].clone(),
        workdir: workdir.clone(),
        inbox: inbox.clone(),
        emit: Arc::new(move |e| {
            sink.lock().unwrap().push(e.clone());
            if let Some(a) = &answer {
                a(&e);
            }
        }),
        clock,
        signal: None,
        tuning,
    }));
    Rig {
        _sb: sb,
        inbox,
        events,
        task: Some(task),
        workdir,
    }
}

const T0: i64 = 1_800_000_000_000;

fn manual() -> Arc<ManualClock> {
    Arc::new(ManualClock::new(T0))
}

/// Moves a manual clock forward in steps until `cond` holds (a sleeper registered a moment after its event still wakes).
async fn advance_until(
    clock: &ManualClock,
    step_ms: i64,
    what: &str,
    mut cond: impl FnMut() -> bool,
) {
    let start = std::time::Instant::now();
    while !cond() {
        assert!(
            start.elapsed() < Duration::from_secs(10),
            "timed out waiting for {what}"
        );
        clock.advance(step_ms).await;
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}

fn data(e: &AgentEvent) -> &Value {
    e.data.as_ref().unwrap()
}

// ---------------------------------------------------------------------------------------------------- rate limits

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rate_limit_that_lifts_soon_is_waited_out_on_the_same_harness() {
    let clock = manual();
    let claude = Harness::new(
        AgentKind::ClaudeCode,
        vec![Turn::RateLimited {
            resets_at_ms: Some(T0 + 10 * 60_000),
        }],
    );
    let codex = Harness::new(AgentKind::Codex, vec![]);
    let mut rig = launch(
        &[claude.clone(), codex.clone()],
        Opts {
            clock: Some(clock.clone()),
            ..Opts::default()
        },
    )
    .await;
    until(|| !rig.harness_events("wait").is_empty(), "the wait").await;
    let wait = &rig.harness_events("wait")[0];
    assert!(
        wait.text
            .as_deref()
            .unwrap()
            .contains("Claude Code is rate limited")
    );
    assert_eq!(data(wait)["harness"]["resumesAt"], T0 + 10 * 60_000);

    // not before it lifts
    clock.advance(9 * 60_000).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(claude.calls(), 1, "still waiting");
    assert_eq!(codex.calls(), 0, "the other harness is not touched");

    advance_until(&clock, 30_000, "the second turn", || claude.calls() >= 2).await;
    assert_eq!(codex.calls(), 0);
    assert!(rig.harness_events("switch").is_empty());
    // the agent is told what happened
    assert!(
        claude
            .message(1)
            .contains("cut short by a rate limit that has lifted")
    );
    rig.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_long_rate_limit_switches_to_the_next_harness_with_the_brief_and_history() {
    let claude = Harness::new(
        AgentKind::ClaudeCode,
        vec![
            Turn::Works,
            Turn::RateLimited {
                resets_at_ms: Some(i64::MAX / 4), // hours from now
            },
        ],
    );
    let codex = Harness::new(AgentKind::Codex, vec![]);
    let mut rig = launch(&[claude.clone(), codex.clone()], Opts::default()).await;
    until(|| codex.starts() >= 1, "the switch").await;

    let switches = rig.harness_events("switch");
    assert_eq!(switches.len(), 1, "one switch, one entry in the run log");
    let h = &data(&switches[0])["harness"];
    assert_eq!(
        (h["from"].as_str(), h["to"].as_str()),
        (Some("claude-code"), Some("codex"))
    );
    assert_eq!(h["cause"], "rate-limited");
    // a rate limit is not announced as a notification; the run log carries it
    assert!(data(&switches[0]).get("notify").is_none());

    // the new CLI starts from the brief and the history, not from the other CLI's session
    let first = codex.message(0);
    assert!(first.contains("Make it faster"), "the brief: {first}");
    assert!(first.contains("taking over from Claude Code"), "{first}");
    assert!(
        first.contains("Verdict for candidate 1"),
        "the history rides along: {first}"
    );
    assert_eq!(claude.calls(), 2, "claude is not asked again");
    rig.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_rate_limit_with_no_reset_time_switches_too() {
    let claude = Harness::new(
        AgentKind::ClaudeCode,
        vec![Turn::RateLimited { resets_at_ms: None }],
    );
    let codex = Harness::new(AgentKind::Codex, vec![]);
    let mut rig = launch(&[claude.clone(), codex.clone()], Opts::default()).await;
    until(|| codex.starts() >= 1, "the switch").await;
    assert!(rig.harness_events("wait").is_empty());
    assert_eq!(rig.harness_events("switch").len(), 1);
    rig.stop().await;
}

// ---------------------------------------------------------------------------------------------------- auth expired

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_expired_sign_in_switches_at_once_and_notifies_once() {
    let codex = Harness::new(AgentKind::Codex, vec![Turn::AuthExpired]);
    let claude = Harness::new(AgentKind::ClaudeCode, vec![]);
    let mut rig = launch(&[codex.clone(), claude.clone()], Opts::default()).await;
    until(|| claude.starts() >= 1, "the switch").await;
    until(|| claude.calls() >= 2, "a second turn").await; // keeps going on the new harness

    let switches = rig.harness_events("switch");
    assert_eq!(switches.len(), 1);
    assert_eq!(data(&switches[0])["harness"]["cause"], "auth-expired");
    assert_eq!(
        data(&switches[0])["notify"],
        "Codex sign-in expired; continuing with Claude Code"
    );
    let notices = rig
        .events()
        .iter()
        .filter(|e| e.data.as_ref().is_some_and(|d| d.get("notify").is_some()))
        .count();
    assert_eq!(notices, 1, "exactly one notification event");
    assert_eq!(
        codex.calls(),
        1,
        "codex is not tried again while signed out"
    );
    rig.stop().await;
}

// ---------------------------------------------------------------------------------------------------- crashes

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn three_crashes_in_a_row_switch_and_two_do_not() {
    let claude = Harness::new(
        AgentKind::ClaudeCode,
        vec![Turn::Crash, Turn::Crash, Turn::Crash],
    );
    let codex = Harness::new(AgentKind::Codex, vec![]);
    let mut rig = launch(&[claude.clone(), codex.clone()], Opts::default()).await;
    until(|| codex.starts() >= 1, "the switch").await;
    assert_eq!(claude.calls(), 3);
    let switches = rig.harness_events("switch");
    assert_eq!(switches.len(), 1);
    assert_eq!(data(&switches[0])["harness"]["cause"], "crashing");
    rig.stop().await;

    // two crashes, then it works: no switch
    let claude = Harness::new(AgentKind::ClaudeCode, vec![Turn::Crash, Turn::Crash]);
    let codex = Harness::new(AgentKind::Codex, vec![]);
    let mut rig = launch(&[claude.clone(), codex.clone()], Opts::default()).await;
    until(|| claude.calls() >= 4, "turns after the crashes").await;
    assert_eq!(codex.calls(), 0);
    assert!(rig.harness_events("switch").is_empty());
    rig.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lone_harness_that_keeps_crashing_still_fails_the_agent() {
    let claude = Harness::new(
        AgentKind::ClaudeCode,
        vec![Turn::Crash, Turn::Crash, Turn::Crash],
    );
    let mut rig = launch(std::slice::from_ref(&claude), Opts::default()).await;
    let res = rig.task.take().unwrap().await.unwrap();
    assert_eq!(res.reason, RunnerStopReason::Failed);
    assert_eq!(claude.calls(), 3);
}

// ---------------------------------------------------------------------------------------------------- all down

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn when_every_harness_is_down_the_optimization_pauses_and_resumes_by_itself() {
    let clock = manual();
    let claude = Harness::new(
        AgentKind::ClaudeCode,
        vec![Turn::RateLimited { resets_at_ms: None }],
    );
    let codex = Harness::new(AgentKind::Codex, vec![]);
    // detection sees neither Codex nor Grok signed in
    let detection = Arc::new(Detection(Mutex::new(vec![AgentKind::ClaudeCode])));
    let mut rig = launch(
        &[claude.clone(), codex.clone()],
        Opts {
            clock: Some(clock.clone()),
            probe: Some(detection.clone()),
            ..Opts::default()
        },
    )
    .await;
    until(|| !rig.harness_events("paused").is_empty(), "the pause").await;
    let paused = &rig.harness_events("paused")[0];
    assert_eq!(data(paused)["state"], "paused", "the agent shows as paused");
    let reason = data(paused)["harness"]["reason"].as_str().unwrap();
    assert!(
        reason.contains("Claude Code: rate limited") && reason.contains("codex is signed out"),
        "{reason}"
    );
    assert!(
        data(paused)["notify"]
            .as_str()
            .unwrap()
            .contains("resumes by itself")
    );
    assert_eq!(codex.calls(), 0);

    // nothing changes for a while: still down at the first re-check (5 min) ...
    clock.advance(5 * 60_000 + 1).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(rig.harness_events("resumed").is_empty());

    // ... then Codex signs in; the next re-check finds it
    *detection.0.lock().unwrap() = vec![AgentKind::ClaudeCode, AgentKind::Codex];
    advance_until(&clock, 60_000, "the resume", || {
        !rig.harness_events("resumed").is_empty()
    })
    .await;
    let resumed = &rig.harness_events("resumed")[0];
    assert_eq!(data(resumed)["state"], "running");
    assert_eq!(data(resumed)["harness"]["harness"], "codex");
    until(|| codex.starts() >= 1, "the new harness's first turn").await;
    assert_eq!(rig.harness_events("switch").len(), 1);
    rig.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_slot_whose_own_harness_is_signed_out_starts_on_the_next_one() {
    let claude = Harness::new(AgentKind::ClaudeCode, vec![]);
    claude.signed_in.store(false, Ordering::SeqCst);
    let codex = Harness::new(AgentKind::Codex, vec![]);
    let mut rig = launch(&[claude.clone(), codex.clone()], Opts::default()).await;
    until(|| codex.starts() >= 1, "codex").await;
    assert_eq!(claude.calls(), 0);
    let switches = rig.harness_events("switch");
    assert_eq!(data(&switches[0])["harness"]["cause"], "unavailable");
    rig.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn with_no_harness_able_to_start_the_agent_is_unavailable_as_before() {
    let claude = Harness::new(AgentKind::ClaudeCode, vec![]);
    claude.signed_in.store(false, Ordering::SeqCst);
    let mut rig = launch(std::slice::from_ref(&claude), Opts::default()).await;
    let res = rig.task.take().unwrap().await.unwrap();
    assert_eq!(res.reason, RunnerStopReason::AgentUnavailable);
}

// ---------------------------------------------------------------------------------------------------- steering notes

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_steering_note_reaches_the_next_turn_and_is_marked_read_by_that_harness() {
    let claude = Harness::new(AgentKind::ClaudeCode, vec![]);
    let mut rig = launch(
        std::slice::from_ref(&claude),
        Opts {
            manual: true,
            ..Opts::default()
        },
    )
    .await;
    until(
        || {
            rig.events()
                .iter()
                .any(|e| e.event_type == AgentEventType::Commit)
        },
        "the first commit",
    )
    .await;
    let sha = rig
        .events()
        .iter()
        .find(|e| e.event_type == AgentEventType::Commit)
        .unwrap()
        .data_str("sha")
        .unwrap()
        .to_owned();

    rig.inbox.push(RunnerMessage::User {
        text: "focus on the filter parser".into(),
    });
    until(
        || !rig.steering("queued").is_empty(),
        "the note to be queued",
    )
    .await;
    assert!(rig.steering("read").is_empty(), "queued, not read yet");
    let queued = &rig.steering("queued")[0];
    assert_eq!(data(queued)["steering"]["seq"], 1);

    rig.inbox.push(RunnerMessage::Verdict {
        sha,
        verdict: CandidateVerdict::Rejected,
        reasons: vec![],
        current_best: 20.0,
    });
    until(|| claude.calls() >= 2, "the second turn").await;
    assert!(
        claude
            .message(1)
            .contains("Message from the user: focus on the filter parser"),
        "{}",
        claude.message(1)
    );
    let reads = rig.steering("read");
    assert_eq!(reads.len(), 1);
    let s = &data(&reads[0])["steering"];
    assert_eq!(s["seq"], 1);
    assert_eq!(s["harness"], "claude-code");
    assert_eq!(s["turn"], 2);
    assert!(s["readAt"].as_i64().unwrap() >= s["queuedAt"].as_i64().unwrap());
    rig.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_note_waiting_when_the_harness_changes_is_read_by_the_new_harness() {
    let claude = Harness::new(AgentKind::ClaudeCode, vec![Turn::Works, Turn::AuthExpired]);
    let codex = Harness::new(AgentKind::Codex, vec![]);
    let mut rig = launch(
        &[claude.clone(), codex.clone()],
        Opts {
            manual: true,
            ..Opts::default()
        },
    )
    .await;
    until(
        || {
            rig.events()
                .iter()
                .any(|e| e.event_type == AgentEventType::Commit)
        },
        "the first commit",
    )
    .await;
    let sha = rig
        .events()
        .iter()
        .find(|e| e.event_type == AgentEventType::Commit)
        .unwrap()
        .data_str("sha")
        .unwrap()
        .to_owned();
    // the note is read by turn 2, which loses the sign-in: its first read is by claude, then codex carries it on
    rig.inbox.push(RunnerMessage::User {
        text: "prefer small diffs".into(),
    });
    until(|| !rig.steering("queued").is_empty(), "the note").await;
    rig.inbox.push(RunnerMessage::Verdict {
        sha,
        verdict: CandidateVerdict::Rejected,
        reasons: vec![],
        current_best: 20.0,
    });
    until(|| codex.starts() >= 1, "codex").await;
    let reads = rig.steering("read");
    assert_eq!(
        reads.len(),
        1,
        "read once, by the harness whose turn took it"
    );
    assert_eq!(data(&reads[0])["steering"]["harness"], "claude-code");
    // the turn that took it failed before the agent could act on it, so the handover repeats it
    assert!(
        codex.message(0).contains("prefer small diffs"),
        "{}",
        codex.message(0)
    );
    rig.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_note_queued_before_the_first_turn_is_in_the_first_prompt() {
    let claude = Harness::new(AgentKind::ClaudeCode, vec![]);
    let sb = Arc::new(make_sandbox());
    drop(sb);
    let mut rig = launch(
        std::slice::from_ref(&claude),
        Opts {
            manual: true,
            clock: Some(manual()),
            ..Opts::default()
        },
    )
    .await;
    // the manual clock never fires the deadline; the loop starts at once, so the note races turn 1: either way it is read
    rig.inbox.push(RunnerMessage::User {
        text: "start with the parser".into(),
    });
    until(
        || claude.calls() >= 1 && !rig.steering("queued").is_empty(),
        "turn 1",
    )
    .await;
    rig.stop().await;
}

// ---------------------------------------------------------------------------------------------------- tools on every turn

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_optimization_tool_set_is_attached_to_every_turn_of_every_harness() {
    let claude = Harness::new(AgentKind::ClaudeCode, vec![Turn::Works, Turn::AuthExpired]);
    let codex = Harness::new(AgentKind::Codex, vec![]);
    let server = McpServer {
        name: "ratchet".into(),
        command: "/bin/ratchetd".into(),
        args: vec!["mcp".into(), "--session".into(), "tok".into()],
        env: Default::default(),
    };
    let mut rig = launch(
        &[claude.clone(), codex.clone()],
        Opts {
            mcp: vec![server.clone()],
            ..Opts::default()
        },
    )
    .await;
    until(|| codex.calls() >= 2, "turns on both harnesses").await;
    rig.stop().await;
    for h in [&claude, &codex] {
        let specs = h.specs.lock().unwrap();
        assert!(!specs.is_empty());
        for spec in specs.iter() {
            assert_eq!(
                spec.mcp.as_slice(),
                std::slice::from_ref(&server),
                "{} turn without the tools",
                h.kind
            );
        }
    }
}

// ---------------------------------------------------------------------------------------------------- submit_candidate

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn submit_candidate_describes_the_commit_and_a_wrong_sha_is_reported() {
    let claude = Harness::new(AgentKind::ClaudeCode, vec![Turn::Works]);
    let mut rig = launch(
        std::slice::from_ref(&claude),
        Opts {
            manual: true,
            ..Opts::default()
        },
    )
    .await;
    // the tool call arrives while the turn runs; the loop reads it when the turn ends
    rig.inbox.push(RunnerMessage::Submit {
        sha: "deadbeef".into(),
        note: Some("Cache the parsed filter".into()),
    });
    until(
        || {
            rig.events()
                .iter()
                .any(|e| e.event_type == AgentEventType::Commit)
        },
        "the commit",
    )
    .await;
    assert!(rig.has_text("submit_candidate named deadbeef but the branch head is"));
    let log = common::sh(&rig.workdir, &["log", "-1", "--format=%B"]);
    assert!(log.contains("ratchet: Cache the parsed filter"), "{log}");
    assert!(log.contains("Ratchet-Candidate: true"));
    rig.stop().await;
}
