mod common;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use common::{
    AutoOptions, Decision, Sandbox, auto_verdicts, base_config, make_sandbox, rule, set_best, sh,
    until, write_file,
};
use ratchet_agents::domain::{
    AgentAdapter, AgentError, AgentEvent, AgentEventType, AgentKind, AgentSession, AgentSpec,
    AgentUsage, CandidateVerdict, EventCallback, Preflight, RunnerMessage,
};
use ratchet_agents::git::{GitCall, GitResult, GitRunner};
use ratchet_agents::workdir::{PrepareWorkdirOptions, prepare_workdir};
use ratchet_agents::{
    AgentScript, ExecGit, LoopTuning, MemoryInbox, RunnerLoopOptions, RunnerLoopResult,
    RunnerStopReason, ScriptStep, ScriptedAgent, SystemClock, run_runner_loop,
};
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;

fn write(path: &str, content: &str) -> ScriptStep {
    ScriptStep::Write {
        path: path.into(),
        content: content.into(),
    }
}

fn script(attempts: Vec<Vec<ScriptStep>>) -> AgentScript {
    AgentScript::new("t", attempts)
}

fn git_step(args: &[&str]) -> ScriptStep {
    ScriptStep::Git {
        args: args.iter().map(|s| s.to_string()).collect(),
    }
}

fn sleep_step(ms: u64) -> ScriptStep {
    ScriptStep::Sleep { ms }
}

fn usage(input_tokens: u64, output_tokens: u64, cost_usd: Option<f64>) -> ScriptStep {
    ScriptStep::Usage {
        input_tokens,
        output_tokens,
        cost_usd,
    }
}

struct Run {
    sb: Arc<Sandbox>,
    inbox: Arc<MemoryInbox>,
    events: Arc<Mutex<Vec<AgentEvent>>>,
    agent: Arc<dyn Received>,
    workdir: PathBuf,
    task: Option<tokio::task::JoinHandle<RunnerLoopResult>>,
}

/// The messages a scripted agent was resumed with (also through wrappers).
trait Received: Send + Sync {
    fn received(&self) -> Vec<String>;
}

impl Received for ScriptedAgent {
    fn received(&self) -> Vec<String> {
        ScriptedAgent::received(self)
    }
}

impl Run {
    async fn result(&mut self) -> RunnerLoopResult {
        self.task
            .take()
            .expect("result taken once")
            .await
            .expect("loop task")
    }
    fn events(&self) -> Vec<AgentEvent> {
        self.events.lock().unwrap().clone()
    }
    fn has_event(&self, f: impl Fn(&AgentEvent) -> bool) -> bool {
        self.events.lock().unwrap().iter().any(f)
    }
    fn has_text(&self, text: &str) -> bool {
        self.has_event(|e| e.text.as_deref() == Some(text))
    }
    fn commits(&self) -> Vec<AgentEvent> {
        self.events()
            .into_iter()
            .filter(|e| e.kind == AgentEventType::Commit)
            .collect()
    }
    fn received(&self) -> Vec<String> {
        self.agent.received()
    }
}

type ConfigTweak = Box<dyn FnOnce(&mut Value)>;
type TuningTweak = Box<dyn FnOnce(&mut LoopTuning)>;

#[derive(Default)]
struct Launch {
    sb: Option<Arc<Sandbox>>,
    rule: Option<common::VerdictRule>,
    manual: bool,
    config: Option<ConfigTweak>,
    tuning: Option<TuningTweak>,
    on_verdict: Option<common::VerdictHook>,
    agent: Option<Arc<dyn AgentAdapter>>,
    received: Option<Arc<dyn Received>>,
    signal: Option<CancellationToken>,
}

impl Launch {
    fn manual() -> Self {
        Self {
            manual: true,
            ..Self::default()
        }
    }
    fn sandbox(mut self, sb: Arc<Sandbox>) -> Self {
        self.sb = Some(sb);
        self
    }
    fn rule(mut self, r: common::VerdictRule) -> Self {
        self.rule = Some(r);
        self
    }
    fn config(mut self, f: impl FnOnce(&mut Value) + 'static) -> Self {
        self.config = Some(Box::new(f));
        self
    }
    fn tuning(mut self, f: impl FnOnce(&mut LoopTuning) + 'static) -> Self {
        self.tuning = Some(Box::new(f));
        self
    }
    fn on_verdict(mut self, f: impl Fn(&str, CandidateVerdict) + Send + Sync + 'static) -> Self {
        self.on_verdict = Some(Arc::new(f));
        self
    }
    fn agent(mut self, adapter: Arc<dyn AgentAdapter>, received: Arc<dyn Received>) -> Self {
        self.agent = Some(adapter);
        self.received = Some(received);
        self
    }
    fn signal(mut self, s: CancellationToken) -> Self {
        self.signal = Some(s);
        self
    }

    async fn go(self, s: AgentScript) -> Run {
        let sb = self.sb.unwrap_or_else(|| Arc::new(make_sandbox()));
        let config = base_config(&sb, |c| {
            if let Some(f) = self.config {
                f(c)
            }
        });
        let workdir = sb.dir.join(format!("work-{}", config.agent_id));
        prepare_workdir(PrepareWorkdirOptions {
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
        let events: Arc<Mutex<Vec<AgentEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let (adapter, received): (Arc<dyn AgentAdapter>, Arc<dyn Received>) =
            match (self.agent, self.received) {
                (Some(a), Some(r)) => (a, r),
                _ => {
                    let agent = Arc::new(ScriptedAgent::new(s));
                    (agent.clone(), agent)
                }
            };
        let answer = (!self.manual).then(|| {
            let push_to = inbox.clone();
            auto_verdicts(
                move |m| push_to.push(m),
                AutoOptions {
                    rule: self.rule.clone(),
                    on_verdict: self.on_verdict.clone(),
                    start: None,
                },
            )
        });
        let sink = events.clone();
        let mut tuning = LoopTuning {
            max_idle_turns: Some(1),
            retry_delay: Some(Duration::from_millis(1)),
            ..LoopTuning::default()
        };
        if let Some(f) = self.tuning {
            f(&mut tuning);
        }
        let task = tokio::spawn(run_runner_loop(RunnerLoopOptions {
            config,
            adapter,
            workdir: workdir.clone(),
            inbox: inbox.clone(),
            emit: Arc::new(move |e| {
                sink.lock().unwrap().push(e.clone());
                if let Some(answer) = &answer {
                    answer(&e);
                }
            }),
            clock: Arc::new(SystemClock),
            signal: self.signal,
            tuning,
        }));
        Run {
            sb,
            inbox,
            events,
            agent: received,
            workdir,
            task: Some(task),
        }
    }
}

fn types(events: &[AgentEvent]) -> Vec<AgentEventType> {
    events.iter().map(|e| e.kind).collect()
}

fn verdicts_of(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e.kind == AgentEventType::Verdict)
        .map(|e| e.data_str("verdict").unwrap().to_string())
        .collect()
}

fn read(dir: &std::path::Path, path: &str) -> String {
    std::fs::read_to_string(dir.join(path)).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn prompt_turn_commit_with_trailer_push_verdict_resume_idle_stop() {
    let mut run = Launch::default()
        .tuning(|t| t.max_idle_turns = Some(2))
        .go(script(vec![
            vec![
                ScriptStep::Message {
                    text: "Trying idea one".into(),
                },
                write("src/app.txt", "line 1\nfast 2\nline 3\n"),
                usage(40_000, 2_000, Some(0.25)),
            ],
            vec![
                ScriptStep::Message {
                    text: "Idea two".into(),
                },
                write("src/extra.txt", "x\n"),
            ],
        ]))
        .await;
    let res = run.result().await;
    assert_eq!(
        (res.reason, res.candidates, res.best_value),
        (RunnerStopReason::Idle, 2, 16.0)
    );
    assert!((res.spend_usd - 0.25).abs() < 1e-9);

    // remote branch has both candidate commits, each with the trailer exactly once
    let engine = &run.sb.engine;
    sh(engine, &["fetch", "--quiet", "origin"]);
    let log = sh(
        engine,
        &["log", "--format=%B%x01", "main..origin/ratchet/a"],
    );
    let msgs: Vec<&str> = log
        .split('\x01')
        .map(str::trim)
        .filter(|m| !m.is_empty())
        .collect();
    assert_eq!(msgs.len(), 2);
    for m in &msgs {
        assert_eq!(m.matches("Ratchet-Candidate: true").count(), 1);
    }
    assert!(msgs[1].contains("ratchet: Trying idea one"));
    assert!(sh(engine, &["show", "origin/ratchet/a:src/app.txt"]).contains("fast 2"));

    // events: status, agent events, commit, verdict
    let events = run.events();
    let commits = run.commits();
    assert_eq!(commits.len(), 2);
    let data = commits[0].data.as_ref().unwrap();
    assert_eq!(
        (&data["branch"], &data["files"]),
        (&json!("ratchet/a"), &json!(["src/app.txt"]))
    );
    assert_eq!(verdicts_of(&events), vec!["verified", "verified"]);
    let ts = types(&events);
    assert!(
        ts.iter().position(|t| *t == AgentEventType::Commit)
            < ts.iter().position(|t| *t == AgentEventType::Verdict)
    );
    let last = events.last().unwrap();
    assert_eq!(
        (last.kind, last.text.as_deref()),
        (AgentEventType::Status, Some("runner finished: idle"))
    );

    // the second turn got a resume message with the verdict, best and limits
    let received = run.received();
    assert!(received[0].contains("Verdict for candidate 1"));
    assert!(received[0].contains("VERIFIED"));
    assert!(received[0].contains("Current ratchet best: 18"));
    assert!(received[0].contains("Budget remaining: $49.75"));
    assert!(received[1].contains("Verdict for candidate 2"));
    assert!(received[2].contains("No change to the repository was detected"));

    // context.md exists in the workdir, untracked and unstaged
    let ctx = read(&run.workdir, ".ratchet/context.md");
    assert!(ctx.contains("http_req_duration.p95"));
    assert!(ctx.contains("| 1 |"));
    assert_eq!(sh(&run.workdir, &["status", "--porcelain"]), "");
    assert_eq!(sh(&run.workdir, &["ls-files", ".ratchet"]), "");
}

/// Records the prompt of every `start`.
struct PromptSpy {
    inner: Arc<ScriptedAgent>,
    prompts: Mutex<Vec<String>>,
}

#[async_trait]
impl AgentAdapter for PromptSpy {
    fn kind(&self) -> AgentKind {
        self.inner.kind()
    }
    async fn preflight(&self) -> Preflight {
        self.inner.preflight().await
    }
    async fn start(
        &self,
        spec: &AgentSpec,
        on_event: EventCallback<'_>,
        cancel: &CancellationToken,
    ) -> Result<AgentSession, AgentError> {
        self.prompts.lock().unwrap().push(spec.prompt.clone());
        self.inner.start(spec, on_event, cancel).await
    }
    async fn resume(
        &self,
        session: &mut AgentSession,
        spec: &AgentSpec,
        message: &str,
        on_event: EventCallback<'_>,
        cancel: &CancellationToken,
    ) -> Result<(), AgentError> {
        self.inner
            .resume(session, spec, message, on_event, cancel)
            .await
    }
}

impl Received for PromptSpy {
    fn received(&self) -> Vec<String> {
        self.inner.received()
    }
}

/// An adapter that reports tokens but no dollars.
struct TokensOnly(Arc<ScriptedAgent>);

#[async_trait]
impl AgentAdapter for TokensOnly {
    fn kind(&self) -> AgentKind {
        self.0.kind()
    }
    async fn preflight(&self) -> Preflight {
        self.0.preflight().await
    }
    async fn start(
        &self,
        spec: &AgentSpec,
        on_event: EventCallback<'_>,
        cancel: &CancellationToken,
    ) -> Result<AgentSession, AgentError> {
        let strip = |mut e: AgentEvent| {
            if let Some(u) = e.usage.as_mut() {
                *u = AgentUsage {
                    input_tokens: u.input_tokens,
                    output_tokens: u.output_tokens,
                    cost_usd: None,
                };
            }
            on_event(e)
        };
        self.0.start(spec, &strip, cancel).await
    }
    async fn resume(
        &self,
        session: &mut AgentSession,
        spec: &AgentSpec,
        message: &str,
        on_event: EventCallback<'_>,
        cancel: &CancellationToken,
    ) -> Result<(), AgentError> {
        self.0
            .resume(session, spec, message, on_event, cancel)
            .await
    }
}

impl Received for TokensOnly {
    fn received(&self) -> Vec<String> {
        self.0.received()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_first_prompt_carries_the_ratchet_md_brief_and_the_generated_context() {
    let spy = Arc::new(PromptSpy {
        inner: Arc::new(ScriptedAgent::new(script(vec![vec![]]))),
        prompts: Mutex::new(Vec::new()),
    });
    let mut run = Launch::default()
        .agent(spy.clone(), spy.clone())
        .go(script(vec![]))
        .await;
    run.result().await;
    let prompts = spy.prompts.lock().unwrap();
    let p = &prompts[0];
    for needle in [
        "# Brief",
        "Make it faster",
        "## Objective",
        "Baseline: 20",
        "Protected paths",
        "`bench/**`",
        "`pnpm test` must pass",
        "`error_rate` < 0.001",
        "Budget remaining: $50.00",
    ] {
        assert!(p.contains(needle), "{needle} missing from {p}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_commit_the_agent_made_itself_gets_the_trailer_added_message_kept() {
    let mut run = Launch::default()
        .go(script(vec![vec![
            write("src/app.txt", "changed\n"),
            git_step(&["add", "-A"]),
            git_step(&[
                "commit",
                "-q",
                "-m",
                "agent: memoize parser\n\nmeasured -12%",
            ]),
        ]]))
        .await;
    run.result().await;
    sh(&run.sb.engine, &["fetch", "--quiet", "origin"]);
    assert_eq!(
        sh(
            &run.sb.engine,
            &["log", "-1", "--format=%B", "origin/ratchet/a"]
        ),
        "agent: memoize parser\n\nmeasured -12%\n\nRatchet-Candidate: true"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dirty_tree_on_top_of_an_agent_commit_is_committed_too_one_candidate_not_two() {
    let mut run = Launch::default()
        .go(script(vec![vec![
            write("a.txt", "1\n"),
            git_step(&["add", "-A"]),
            git_step(&["commit", "-q", "-m", "first"]),
            write("b.txt", "2\n"),
        ]]))
        .await;
    run.result().await;
    assert_eq!(run.commits().len(), 1);
    sh(&run.sb.engine, &["fetch", "--quiet", "origin"]);
    assert_eq!(
        sh(
            &run.sb.engine,
            &["show", "--name-only", "--format=", "origin/ratchet/a"]
        ),
        "b.txt"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn records_rejected_and_blocked_verdicts_tells_the_agent_and_refs_ratchet_accepted_lets_it_drop_the_work()
 {
    let verdict_rule = rule(|c| match c.n {
        1 => Some(
            Decision::new(CandidateVerdict::Blocked)
                .with_reasons(&[&format!("touches protected path {}", c.files[0])]),
        ),
        2 => {
            Some(Decision::new(CandidateVerdict::Rejected).with_reasons(&["gain 0.4% below noise"]))
        }
        _ => None,
    });
    let mut run = Launch::default()
        .rule(verdict_rule)
        .go(script(vec![
            vec![write("bench/run.sh", "echo cheat\n")],
            vec![
                git_step(&["reset", "--hard", "refs/ratchet/accepted"]),
                write("src/app.txt", "noise\n"),
            ],
            vec![
                git_step(&["reset", "--hard", "refs/ratchet/accepted"]),
                write("src/app.txt", "real\n"),
            ],
        ]))
        .await;
    let res = run.result().await;
    assert_eq!(
        res.history.iter().map(|h| h.verdict).collect::<Vec<_>>(),
        vec![
            CandidateVerdict::Blocked,
            CandidateVerdict::Rejected,
            CandidateVerdict::Verified
        ]
    );
    let received = run.received();
    assert!(received[0].contains("BLOCKED"));
    assert!(received[0].contains("touches protected path bench/run.sh"));
    assert!(received[0].contains("git reset --hard refs/ratchet/accepted"));
    // the cheat was dropped: the final tree has the original bench script
    assert_eq!(read(&run.workdir, "bench/run.sh"), "echo bench\n");
    assert_eq!(read(&run.workdir, "src/app.txt"), "real\n");
    assert_eq!(res.best_value, 18.0); // only the verified one moved it
    // a later candidate's `files` is relative to the accepted tree, not the whole branch history
    let files: Vec<Value> = run
        .commits()
        .iter()
        .map(|e| e.data.as_ref().unwrap()["files"].clone())
        .collect();
    assert_eq!(
        files,
        vec![
            json!(["bench/run.sh"]),
            json!(["src/app.txt"]),
            json!(["src/app.txt"])
        ]
    );
}

fn make_best(sb: &Sandbox, with_extra_file: bool) -> String {
    sh(
        &sb.engine,
        &["checkout", "-q", "-b", "other", "origin/main"],
    );
    write_file(&sb.engine, "src/app.txt", "line 1\nBEST 2\nline 3\n");
    if with_extra_file {
        write_file(&sb.engine, "src/best.txt", "best\n");
    }
    sh(&sb.engine, &["add", "-A"]);
    sh(&sb.engine, &["commit", "-q", "-m", "best"]);
    sh(&sb.engine, &["rev-parse", "HEAD"])
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rebases_onto_the_ratchet_best_when_it_moved_preferring_the_best_on_conflict_and_tells_the_agent()
 {
    let sb = Arc::new(make_sandbox());
    // another agent's verified commit becomes best: edits line 2 and adds a file
    let best_sha = make_best(&sb, true);
    // our agent: conflicting edit of line 2 plus a separate file, rejected
    let (sb2, best2) = (sb.clone(), best_sha.clone());
    let mut run = Launch::default()
        .sandbox(sb.clone())
        .rule(rule(|_| {
            Some(Decision::new(CandidateVerdict::Rejected).with_reasons(&["below noise"]))
        }))
        .on_verdict(move |_, _| set_best(&sb2, &best2))
        .go(script(vec![
            vec![
                write("src/app.txt", "line 1\nMINE 2\nline 3\n"),
                write("src/mine.txt", "mine\n"),
            ],
            vec![],
        ]))
        .await;
    run.result().await;
    assert_eq!(
        read(&run.workdir, "src/app.txt"),
        "line 1\nBEST 2\nline 3\n"
    ); // best wins the conflict
    assert_eq!(read(&run.workdir, "src/best.txt"), "best\n");
    assert_eq!(read(&run.workdir, "src/mine.txt"), "mine\n"); // non-conflicting work survives
    sh(
        &run.workdir,
        &["merge-base", "--is-ancestor", &best_sha, "HEAD"],
    );
    assert_eq!(
        sh(&run.workdir, &["rev-parse", "refs/ratchet/accepted"]),
        best_sha
    );
    let first = &run.received()[0];
    assert!(
        first.contains("ratchet best moved")
            && first
                .contains("rebased onto it; where they conflicted, the best's version was kept"),
        "{first}"
    );
    assert!(run.has_event(|e| {
        e.kind == AgentEventType::Status
            && e.text
                .as_deref()
                .unwrap_or("")
                .contains("rebased onto ratchet best")
    }));
    assert!(!run.workdir.join(".git/rebase-merge").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn when_the_rebase_cannot_be_resolved_modify_delete_it_resets_to_the_best_and_says_so() {
    let sb = Arc::new(make_sandbox());
    let best_sha = make_best(&sb, false);
    let (sb2, best2) = (sb.clone(), best_sha.clone());
    let mut run = Launch::default()
        .sandbox(sb.clone())
        .rule(rule(|_| Some(Decision::new(CandidateVerdict::Rejected))))
        .on_verdict(move |_, _| set_best(&sb2, &best2))
        .go(script(vec![
            vec![ScriptStep::Delete {
                path: "src/app.txt".into(),
            }],
            vec![],
        ]))
        .await;
    run.result().await;
    assert_eq!(sh(&run.workdir, &["rev-parse", "HEAD"]), best_sha);
    assert!(read(&run.workdir, "src/app.txt").contains("BEST 2"));
    assert!(run.received()[0].contains("conflicted, so your branch was reset to the best"));
    assert!(!run.workdir.join(".git/rebase-merge").exists());
}

fn commit_sha(run: &Run) -> String {
    run.commits()[0].data_str("sha").unwrap().to_string()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn pause_finishes_the_turn_waits_and_only_resumes_the_agent_after_resume() {
    let mut run = Launch::manual()
        .go(script(vec![
            vec![write("a.txt", "1\n")],
            vec![write("b.txt", "2\n")],
        ]))
        .await;
    until(|| !run.commits().is_empty(), "the first commit").await;
    let sha = commit_sha(&run);
    run.inbox.push(RunnerMessage::Pause);
    run.inbox.push(RunnerMessage::Verdict {
        sha,
        verdict: CandidateVerdict::Verified,
        reasons: vec![],
        current_best: 17.0,
    });
    until(|| run.has_text("paused"), "paused").await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(run.received().is_empty()); // verdict recorded, agent not resumed
    run.inbox.push(RunnerMessage::Resume);
    until(|| run.received().len() == 1, "the resume").await;
    assert!(run.received()[0].contains("VERIFIED"));
    assert!(run.has_text("resumed"));
    run.inbox.push(RunnerMessage::Stop);
    assert_eq!(run.result().await.reason, RunnerStopReason::Stopped);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn user_messages_are_delivered_with_the_next_resume() {
    let mut run = Launch::manual()
        .go(script(vec![
            vec![write("a.txt", "1\n")],
            vec![write("b.txt", "2\n")],
        ]))
        .await;
    until(|| !run.commits().is_empty(), "the first commit").await;
    let sha = commit_sha(&run);
    run.inbox.push(RunnerMessage::User {
        text: "focus on the filter parser please".into(),
    });
    run.inbox.push(RunnerMessage::Verdict {
        sha,
        verdict: CandidateVerdict::Rejected,
        reasons: vec!["x".into()],
        current_best: 20.0,
    });
    until(|| run.received().len() == 1, "the resume").await;
    assert!(run.received()[0].contains("Message from the user: focus on the filter parser please"));
    run.inbox.push(RunnerMessage::Stop);
    run.result().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_mid_turn_aborts_the_agent_promptly_and_commits_nothing() {
    let mut run = Launch::manual()
        .go(script(vec![vec![
            write("a.txt", "1\n"),
            sleep_step(60_000),
        ]]))
        .await;
    until(|| run.has_text("agent turn started"), "the turn").await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let t0 = Instant::now();
    run.inbox.push(RunnerMessage::Stop);
    let res = run.result().await;
    assert!(t0.elapsed() < Duration::from_secs(2));
    assert_eq!((res.reason, res.candidates), (RunnerStopReason::Stopped, 0));
    assert!(run.commits().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn stop_while_waiting_for_a_verdict() {
    let mut run = Launch::manual()
        .go(script(vec![vec![write("a.txt", "1\n")]]))
        .await;
    until(
        || run.has_text("waiting for verdict"),
        "waiting for a verdict",
    )
    .await;
    run.inbox.push(RunnerMessage::Stop);
    assert_eq!(run.result().await.reason, RunnerStopReason::Stopped);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_external_abort_signal_ends_the_loop_as_aborted() {
    let cancel = CancellationToken::new();
    let mut run = Launch::manual()
        .signal(cancel.clone())
        .go(script(vec![vec![sleep_step(60_000)]]))
        .await;
    until(|| run.has_text("agent turn started"), "the turn").await;
    cancel.cancel();
    assert_eq!(run.result().await.reason, RunnerStopReason::Aborted);
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn deadline_stops_mid_turn_and_while_waiting_for_a_verdict() {
    let deadline = now_ms() + 300;
    let mut mid_turn = Launch::manual()
        .config(move |c| c["deadlineAt"] = deadline.into())
        .go(script(vec![vec![sleep_step(60_000)]]))
        .await;
    assert_eq!(mid_turn.result().await.reason, RunnerStopReason::Deadline);
    let deadline = now_ms() + 2500;
    let mut waiting = Launch::manual()
        .config(move |c| c["deadlineAt"] = deadline.into())
        .go(script(vec![vec![write("a.txt", "1\n")]]))
        .await;
    assert_eq!(waiting.result().await.reason, RunnerStopReason::Deadline);
    assert!(!waiting.commits().is_empty()); // it did submit before the deadline
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn budget_the_turn_that_crosses_it_is_submitted_and_judged_then_the_loop_stops_without_resuming()
 {
    let mut run = Launch::default()
        .config(|c| c["budgetUsd"] = 0.5.into())
        .go(script(vec![
            vec![write("a.txt", "1\n"), usage(90_000, 5_000, Some(0.6))],
            vec![write("b.txt", "2\n")],
        ]))
        .await;
    let res = run.result().await;
    assert_eq!((res.reason, res.candidates), (RunnerStopReason::Budget, 1));
    assert!((res.spend_usd - 0.6).abs() < 1e-9);
    assert!(run.received().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn budget_can_be_estimated_from_tokens_when_the_adapter_reports_no_cost() {
    let inner = Arc::new(ScriptedAgent::new(script(vec![vec![
        write("a.txt", "1\n"),
        usage(1_000_000, 0, None),
    ]])));
    let adapter = Arc::new(TokensOnly(inner));
    let mut run = Launch::default()
        .agent(adapter.clone(), adapter)
        .config(|c| c["budgetUsd"] = 1.into())
        .tuning(|t| {
            t.estimate_cost = Some(Arc::new(|u: &AgentUsage| u.input_tokens as f64 / 1e6 * 3.0))
        })
        .go(script(vec![]))
        .await;
    let res = run.result().await;
    assert_eq!(res.reason, RunnerStopReason::Budget);
    assert!((res.spend_usd - 3.0).abs() < 1e-9);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn retries_a_crashed_turn_telling_the_agent_and_gives_up_after_max_consecutive_failures() {
    let fail = |m: &str| ScriptStep::Fail { message: m.into() };
    let mut flaky = Launch::default()
        .tuning(|t| t.max_consecutive_failures = Some(3))
        .go(script(vec![
            vec![fail("segfault")],
            vec![write("a.txt", "1\n")],
        ]))
        .await;
    // start crashed -> retried as a fresh start (attempt index advanced by the scripted agent)
    let res = flaky.result().await;
    assert_eq!(
        flaky
            .events()
            .iter()
            .filter(|e| e.kind == AgentEventType::Error
                && e.text
                    .as_deref()
                    .unwrap_or("")
                    .contains("agent turn failed: segfault"))
            .count(),
        1
    );
    assert_eq!(res.candidates, 1);

    let mut doomed = Launch::default()
        .tuning(|t| t.max_consecutive_failures = Some(3))
        .go(script(vec![
            vec![fail("a")],
            vec![fail("b")],
            vec![fail("c")],
        ]))
        .await;
    assert_eq!(doomed.result().await.reason, RunnerStopReason::Failed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn agent_preflight_failure_ends_the_run_as_agent_unavailable() {
    let mut run = Launch::default().go(script(vec![])).await;
    let res = run.result().await;
    assert_eq!(res.reason, RunnerStopReason::AgentUnavailable);
    let error = run
        .events()
        .into_iter()
        .find(|e| e.kind == AgentEventType::Error)
        .unwrap();
    assert!(error.text.unwrap().contains("script has no attempts"));
}

/// Real git, audited, with a configurable number of push failures.
struct AuditGit {
    calls: Mutex<Vec<Vec<String>>>,
    push_failures: Mutex<u32>,
    deny_pushes: bool,
}

#[async_trait]
impl GitRunner for AuditGit {
    async fn run(&self, args: &[String], call: &GitCall) -> GitResult {
        self.calls.lock().unwrap().push(args.to_vec());
        if args.first().map(String::as_str) == Some("push") {
            let mut left = self.push_failures.lock().unwrap();
            if self.deny_pushes {
                return GitResult {
                    code: 1,
                    stdout: String::new(),
                    stderr: "denied".into(),
                };
            }
            if *left > 0 {
                *left -= 1;
                return GitResult {
                    code: 1,
                    stdout: String::new(),
                    stderr: "remote: temporary failure".into(),
                };
            }
        }
        ExecGit.run(args, call).await
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn push_is_retried_and_git_is_injectable_and_audited() {
    let audit = Arc::new(AuditGit {
        calls: Mutex::new(Vec::new()),
        push_failures: Mutex::new(1),
        deny_pushes: false,
    });
    let git = audit.clone();
    let mut run = Launch::default()
        .tuning(move |t| {
            t.git = Some(git);
            t.push_retries = Some(2);
            t.push_retry_delay = Some(Duration::from_millis(1));
        })
        .go(script(vec![vec![write("a.txt", "1\n")]]))
        .await;
    let res = run.result().await;
    assert_eq!(res.candidates, 1);
    let calls = audit.calls.lock().unwrap();
    let pushes: Vec<&Vec<String>> = calls.iter().filter(|c| c[0] == "push").collect();
    assert_eq!(pushes.len(), 2);
    assert_eq!(
        pushes[0],
        &[
            "push",
            "--quiet",
            "--force-with-lease",
            "origin",
            "HEAD:refs/heads/ratchet/a"
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn gives_up_with_failed_when_push_never_succeeds() {
    let audit = Arc::new(AuditGit {
        calls: Mutex::new(Vec::new()),
        push_failures: Mutex::new(0),
        deny_pushes: true,
    });
    let git = audit.clone();
    let mut run = Launch::manual()
        .tuning(move |t| {
            t.git = Some(git);
            t.push_retries = Some(1);
            t.push_retry_delay = Some(Duration::from_millis(1));
        })
        .go(script(vec![vec![write("a.txt", "1\n")]]))
        .await;
    let res = run.result().await;
    assert_eq!(res.reason, RunnerStopReason::Failed);
    assert!(run.has_event(|e| {
        e.kind == AgentEventType::Error
            && e.text
                .as_deref()
                .unwrap_or("")
                .contains("push failed: denied")
    }));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unknown_runner_messages_are_reported_not_fatal() {
    let mut run = Launch::manual()
        .go(script(vec![vec![write("a.txt", "1\n")]]))
        .await;
    until(
        || run.has_text("waiting for verdict"),
        "waiting for a verdict",
    )
    .await;
    run.inbox.push(RunnerMessage::Unknown);
    until(|| run.has_text("unknown runner message"), "the report").await;
    run.inbox.push(RunnerMessage::Stop);
    assert_eq!(run.result().await.reason, RunnerStopReason::Stopped);
}
