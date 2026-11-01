//! The runner loop: owns one agent on one branch.
//!
//! prompt, agent turn, commit (`Ratchet-Candidate: true`), push, wait for the verdict, sync to the ratchet best, resume the agent
//! with the verdict, repeat until stop / deadline / budget / idle.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use ratchet_core::enums::CandidateVerdict;
use ratchet_core::ports::{
    AgentAdapter, AgentEvent, AgentEventType, AgentSession, AgentSpec, AgentUsage,
    CancellationToken, Clock, PortError, PortResult, Preflight, RunnerConfig, RunnerInbox,
    RunnerMessage, SharedEventSink,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::io::AsyncWriteExt;
use tokio::sync::watch;

use crate::context::{
    ContextInput, HistoryEntry, ResumeInput, build_context, build_initial_prompt,
    build_resume_message,
};
use crate::git::{Git, SharedGit, exec_git, has_candidate_trailer, with_candidate_trailer};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunnerStopReason {
    #[serde(rename = "stopped")]
    Stopped,
    #[serde(rename = "deadline")]
    Deadline,
    #[serde(rename = "budget")]
    Budget,
    #[serde(rename = "idle")]
    Idle,
    #[serde(rename = "aborted")]
    Aborted,
    #[serde(rename = "failed")]
    Failed,
    #[serde(rename = "agent-unavailable")]
    AgentUnavailable,
}

impl RunnerStopReason {
    pub fn as_str(self) -> &'static str {
        match self {
            RunnerStopReason::Stopped => "stopped",
            RunnerStopReason::Deadline => "deadline",
            RunnerStopReason::Budget => "budget",
            RunnerStopReason::Idle => "idle",
            RunnerStopReason::Aborted => "aborted",
            RunnerStopReason::Failed => "failed",
            RunnerStopReason::AgentUnavailable => "agent-unavailable",
        }
    }
}

impl fmt::Display for RunnerStopReason {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Estimates the dollar cost of usage that carries only token counts.
pub type CostEstimator = Arc<dyn Fn(&AgentUsage) -> f64 + Send + Sync>;

/// Everything beyond the required wiring; every field is optional and defaults like the TypeScript loop.
#[derive(Clone, Default)]
pub struct LoopTuning {
    /// Injectable git (default: the `git` CLI).
    pub git: Option<SharedGit>,
    /// Extra environment for git, e.g. credentials.
    pub git_env: Option<HashMap<String, String>>,
    /// Remote branch where the engine keeps the ratchet best. Default `ratchet/best`.
    pub best_ref: Option<String>,
    pub remote: Option<String>,
    /// Local ref the loop uses to remember the last verified commit. Default `refs/ratchet/accepted`. Agents that share one clone (git
    /// worktrees) need one ref each, because refs outside `HEAD` are shared by all worktrees.
    pub accepted_ref: Option<String>,
    /// Local tracking ref the best is fetched into. Default `refs/remotes/<remote>/<best_ref>`; per agent for shared clones, like `accepted_ref`.
    pub best_tracking_ref: Option<String>,
    /// Consecutive turns without a change before the loop gives up. Default 2.
    pub max_idle_turns: Option<u32>,
    /// Consecutive crashed agent turns before the loop gives up. Default 3.
    pub max_consecutive_failures: Option<u32>,
    /// Pause between a crashed turn and the retry. Default 5 s.
    pub retry_delay: Option<Duration>,
    pub push_retries: Option<u32>,
    /// Base delay for push retries (doubles each attempt). Default 1 s.
    pub push_retry_delay: Option<Duration>,
    /// For adapters that report tokens but no dollars.
    pub estimate_cost: Option<CostEstimator>,
    pub model: Option<String>,
    pub max_turns: Option<u32>,
    pub agent_env: Option<BTreeMap<String, String>>,
}

/// What [`run_runner_loop`] needs: the wiring of the TypeScript `RunnerLoopOptions` plus the loop's tuning (the TS
/// `RunnerLoopDeps`).
pub struct RunnerLoopOptions {
    pub config: RunnerConfig,
    pub adapter: Arc<dyn AgentAdapter>,
    /// A git clone of `config.repo_url` checked out on `config.branch`, with push access.
    pub workdir: PathBuf,
    pub inbox: Arc<dyn RunnerInbox>,
    pub emit: SharedEventSink,
    pub clock: Arc<dyn Clock>,
    /// External abort: ends the loop as `aborted`.
    pub signal: Option<CancellationToken>,
    pub tuning: LoopTuning,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunnerLoopResult {
    pub reason: RunnerStopReason,
    pub turns: u32,
    pub candidates: usize,
    pub spend_usd: f64,
    pub best_value: f64,
    pub history: Vec<HistoryEntry>,
}

// ---------------------------------------------------------------- mailbox

#[derive(Debug, Clone)]
struct VerdictMsg {
    verdict: CandidateVerdict,
    reasons: Vec<String>,
    current_best: f64,
}

#[derive(Debug, Clone)]
struct LatestBest {
    sha: String,
    branch: String,
}

#[derive(Default)]
struct MailboxState {
    verdicts: HashMap<String, VerdictMsg>,
    user_messages: Vec<String>,
    paused: bool,
    /// Latest shared best announced by the engine (`ratchet` message) or won by this agent.
    latest_best: Option<LatestBest>,
}

/// Buffers inbox messages so a verdict that arrives early (or during a turn) is never lost.
struct Mailbox {
    state: Mutex<MailboxState>,
    version: watch::Sender<u64>,
    cancel: CancellationToken,
}

impl Mailbox {
    fn bump(&self) {
        self.version.send_modify(|v| *v += 1);
    }

    fn subscribe(&self) -> watch::Receiver<u64> {
        self.version.subscribe()
    }

    /// Resolves on the next inbox message, or when `signal` aborts. Subscribe first, check the condition, then wait: a message that
    /// arrives in between is not missed.
    async fn changed(rx: &mut watch::Receiver<u64>, signal: &CancellationToken) {
        tokio::select! {
            _ = rx.changed() => {}
            _ = signal.cancelled() => {}
        }
    }

    fn close(&self) {
        self.cancel.cancel();
        self.bump();
    }
}

// ---------------------------------------------------------------- the loop

#[derive(Default)]
struct Shared {
    spend_usd: f64,
    last_message: String,
    over_budget: bool,
}

struct Core {
    config: RunnerConfig,
    workdir: PathBuf,
    git: Git,
    clock: Arc<dyn Clock>,
    emit_fn: SharedEventSink,
    ctl: CancellationToken,
    reason: Mutex<Option<RunnerStopReason>>,
    mailbox: Arc<Mailbox>,
    shared: Mutex<Shared>,
    tuning: LoopTuning,
    remote: String,
    best_branch: String,
    best_tracking: String,
    accepted_ref: String,
}

struct Local {
    best: f64,
    turns: u32,
    history: Vec<HistoryEntry>,
    best_sha: Option<String>,
}

struct Candidate {
    sha: String,
    files: Vec<String>,
    subject: String,
}

fn first_line(s: &str, max: usize) -> String {
    s.split('\n')
        .next()
        .unwrap_or("")
        .trim()
        .chars()
        .take(max)
        .collect()
}

fn describe(err: &PortError) -> String {
    err.to_string()
}

impl Core {
    fn emit(&self, kind: AgentEventType, text: Option<String>, data: Option<Value>) {
        let mut e = AgentEvent::new(self.clock.now(), kind);
        e.text = text;
        if let Some(d) = data {
            e = e.with_data(d);
        }
        (self.emit_fn)(e);
    }

    fn status(&self, text: impl Into<String>, data: Option<Value>) {
        self.emit(AgentEventType::Status, Some(text.into()), data);
    }

    /// The first reason wins; every later `halt` only repeats the abort.
    fn halt(&self, reason: RunnerStopReason) {
        self.reason.lock().unwrap().get_or_insert(reason);
        self.ctl.cancel();
    }

    fn set_reason(&self, reason: RunnerStopReason) {
        *self.reason.lock().unwrap() = Some(reason);
    }

    fn reason_or(&self, default: RunnerStopReason) -> RunnerStopReason {
        self.reason.lock().unwrap().unwrap_or(default)
    }

    async fn wait_unpaused(&self) {
        if !self.mailbox.state.lock().unwrap().paused {
            return;
        }
        self.status("paused", None);
        loop {
            let mut rx = self.mailbox.subscribe();
            if !self.mailbox.state.lock().unwrap().paused || self.ctl.is_cancelled() {
                break;
            }
            Mailbox::changed(&mut rx, &self.ctl).await;
        }
        if !self.ctl.is_cancelled() {
            self.status("resumed", None);
        }
    }

    async fn write_context(&self, local: &Local, turn: u32) -> Result<String, String> {
        let spend = self.shared.lock().unwrap().spend_usd;
        let best_sha: Option<String> = local.best_sha.as_ref().map(|s| s.chars().take(8).collect());
        let text = build_context(&ContextInput {
            config: &self.config,
            best_value: local.best,
            history: &local.history,
            now: self.clock.now(),
            spend_usd: spend,
            turn,
            best_sha: best_sha.as_deref(),
        });
        let dir = self.workdir.join(".ratchet");
        tokio::fs::create_dir_all(&dir)
            .await
            .map_err(|e| e.to_string())?;
        tokio::fs::write(dir.join("context.md"), &text)
            .await
            .map_err(|e| e.to_string())?;
        Ok(text)
    }

    fn on_agent_event(&self, e: AgentEvent) {
        {
            let mut shared = self.shared.lock().unwrap();
            if e.event_type == AgentEventType::Message
                && let Some(text) = e.text.as_ref().filter(|t| !t.is_empty())
            {
                shared.last_message = text.clone();
            }
            if let (AgentEventType::Usage, Some(usage)) = (e.event_type, e.usage.as_ref()) {
                shared.spend_usd += usage
                    .cost_usd
                    .or_else(|| self.tuning.estimate_cost.as_ref().map(|f| f(usage)))
                    .unwrap_or(0.0);
                if shared.spend_usd >= self.config.budget_usd {
                    shared.over_budget = true;
                }
            }
        }
        (self.emit_fn)(e);
    }

    async fn commit_if_changed(
        &self,
        turn: u32,
        known: &HashSet<String>,
    ) -> Result<Option<Candidate>, String> {
        let git = &self.git;
        let g = |e: crate::git::GitError| e.to_string();
        git.run(&["add", "-A"]).await.map_err(g)?;
        let staged = git.try_run(&["diff", "--cached", "--quiet"]).await.code != 0;
        if staged {
            let last_message = self.shared.lock().unwrap().last_message.clone();
            let subject = Some(first_line(&last_message, 72))
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| format!("candidate {turn}"));
            let trimmed = last_message.trim();
            let body = if trimmed.is_empty() {
                String::new()
            } else {
                format!("\n\n{trimmed}")
            };
            git.run(&[
                "commit",
                "--quiet",
                "--no-verify",
                "-m",
                &with_candidate_trailer(&format!("ratchet: {subject}{body}")),
            ])
            .await
            .map_err(g)?;
        } else {
            let head = git.head().await.map_err(g)?;
            if known.contains(&head) {
                return Ok(None);
            }
            let message = git.message("HEAD").await.map_err(g)?;
            if !has_candidate_trailer(&message) {
                git.run(&[
                    "commit",
                    "--quiet",
                    "--amend",
                    "--no-verify",
                    "-m",
                    &with_candidate_trailer(&message),
                ])
                .await
                .map_err(g)?;
            }
        }
        let sha = git.head().await.map_err(g)?;
        if known.contains(&sha) {
            return Ok(None);
        }
        let parent = git.rev_parse("HEAD~1").await.unwrap_or_else(|| sha.clone());
        let accepted = git.rev_parse(&self.accepted_ref).await.unwrap_or(parent);
        let files = git.changed_files(&accepted, &sha).await.unwrap_or_default();
        let subject = first_line(&git.message("HEAD").await.map_err(g)?, 100);
        Ok(Some(Candidate {
            sha,
            files,
            subject,
        }))
    }

    async fn push(&self) -> bool {
        let attempts = self.tuning.push_retries.unwrap_or(3) + 1;
        let delay = self
            .tuning
            .push_retry_delay
            .unwrap_or(Duration::from_millis(1000));
        let target = format!("HEAD:refs/heads/{}", self.config.branch);
        let mut last = String::new();
        for i in 0..attempts {
            if self.ctl.is_cancelled() {
                break;
            }
            let res = self
                .git
                .try_run(&[
                    "push",
                    "--quiet",
                    "--force-with-lease",
                    &self.remote,
                    &target,
                ])
                .await;
            if res.code == 0 {
                return true;
            }
            last = res.stderr.trim().to_string();
            let _ = self
                .clock
                .sleep((delay * 2u32.pow(i)).as_millis() as u64, Some(&self.ctl))
                .await;
        }
        self.emit(
            AgentEventType::Error,
            Some(format!("push failed: {last}")),
            Some(json!({"fatal": true})),
        );
        false
    }

    /// Fetch the ratchet best and rebase onto it when it moved. Returns notes for the agent.
    async fn sync_to_best(&self, local: &mut Local) -> Result<Vec<String>, String> {
        let git = &self.git;
        let g = |e: crate::git::GitError| e.to_string();
        let mut best_sha: Option<String> = None;
        let announced = self.mailbox.state.lock().unwrap().latest_best.clone();
        if let Some(a) = announced {
            // Preferred: the engine told us exactly which commit is the best and on which branch it lives.
            let refspec = format!("+refs/heads/{}:{}", a.branch, self.best_tracking);
            let fetched = git
                .try_run(&["fetch", "--quiet", &self.remote, &refspec])
                .await;
            if fetched.code == 0
                && git
                    .try_run(&["cat-file", "-e", &format!("{}^{{commit}}", a.sha)])
                    .await
                    .code
                    == 0
            {
                best_sha = Some(a.sha);
            }
        }
        if best_sha.is_none() {
            // Fallback: a remote branch holding the best (`ratchet/best` by convention).
            let refspec = format!("+refs/heads/{}:{}", self.best_branch, self.best_tracking);
            let fetched = git
                .try_run(&["fetch", "--quiet", &self.remote, &refspec])
                .await;
            best_sha = if fetched.code == 0 {
                git.rev_parse(&self.best_tracking).await
            } else {
                None
            };
        }
        let Some(best_sha) = best_sha else {
            return Ok(Vec::new());
        };
        local.best_sha = Some(best_sha.clone());
        git.run(&["update-ref", &self.accepted_ref, &best_sha])
            .await
            .map_err(g)?;
        if git.is_ancestor(&best_sha, "HEAD").await {
            return Ok(Vec::new());
        }
        let before = git.head().await.map_err(g)?;
        let mine = git
            .run(&["rev-list", "--count", &format!("{best_sha}..HEAD")])
            .await
            .map_err(g)?
            .trim()
            .to_string();
        let short: String = best_sha.chars().take(8).collect();
        // `-X ours` in a rebase means the branch being rebased onto: conflicts resolve in favour of the best.
        let rebase = git
            .try_run(&["rebase", "--quiet", "-X", "ours", &best_sha])
            .await;
        if rebase.code == 0 {
            let to = git.head().await.map_err(g)?;
            self.status(
                format!("rebased onto ratchet best {short}"),
                Some(
                    json!({"from": before, "to": to, "commits": mine.parse::<u64>().unwrap_or(0)}),
                ),
            );
            return Ok(vec![format!(
                "The ratchet best moved (now {short}, set by an earlier verified candidate). Your {mine} unverified commit(s) were rebased onto it; where they conflicted, the best's version was kept. Re-run the tests and re-measure before trusting your earlier numbers."
            )]);
        }
        git.try_run(&["rebase", "--abort"]).await;
        git.run(&["reset", "--hard", &best_sha]).await.map_err(g)?;
        self.emit(
            AgentEventType::Status,
            Some(format!("rebase onto {short} conflicted; reset to best")),
            Some(json!({"discarded": before})),
        );
        let before_short: String = before.chars().take(8).collect();
        Ok(vec![format!(
            "The ratchet best moved (now {short}) and rebasing your {mine} commit(s) onto it conflicted, so your branch was reset to the best and those commits (last: {before_short}) were discarded. Re-apply the idea on top of the new best if it still makes sense."
        )])
    }

    fn apply(&self, m: RunnerMessage) {
        let mut st = self.mailbox.state.lock().unwrap();
        match m {
            RunnerMessage::Verdict {
                sha,
                verdict,
                reasons,
                current_best,
            } => {
                if verdict != CandidateVerdict::Pending {
                    st.verdicts.insert(
                        sha,
                        VerdictMsg {
                            verdict,
                            reasons,
                            current_best,
                        },
                    );
                }
            }
            RunnerMessage::User { text } => {
                st.user_messages.push(text.clone());
                drop(st);
                self.status("user message received", Some(json!({"text": text})));
            }
            RunnerMessage::Pause => st.paused = true,
            RunnerMessage::Resume => st.paused = false,
            RunnerMessage::Stop => {
                drop(st);
                self.halt(RunnerStopReason::Stopped);
            }
            RunnerMessage::Ratchet { sha, branch, value } => {
                // Another agent's candidate moved the shared ratchet: rebase onto it on the next sync.
                st.latest_best = Some(LatestBest {
                    sha: sha.clone(),
                    branch: branch.clone(),
                });
                drop(st);
                self.status(
                    format!("ratchet moved to {} by {branch}", value),
                    Some(json!({"sha": sha, "branch": branch, "value": value})),
                );
            }
            RunnerMessage::Unknown => {
                drop(st);
                self.emit(
                    AgentEventType::Error,
                    Some("unknown runner message".to_string()),
                    None,
                );
            }
        }
    }
}

/// The inbox pump: applies every message to the mailbox and wakes whoever waits on it.
async fn pump(core: Arc<Core>, inbox: Arc<dyn RunnerInbox>) {
    let cancel = core.mailbox.cancel.clone();
    while !cancel.is_cancelled() {
        match inbox.next(Some(&cancel)).await {
            Ok(Some(message)) => {
                core.apply(message);
                core.mailbox.bump();
            }
            Ok(None) => return,
            Err(err) => {
                tracing::warn!(%err, "reading the inbox failed; retrying");
                let _ = core.clock.sleep(1000, Some(&cancel)).await;
            }
        }
    }
}

async fn append_exclude(workdir: &Path, git: &Git) -> Result<(), String> {
    let out = git
        .run(&["rev-parse", "--git-path", "info/exclude"])
        .await
        .map_err(|e| e.to_string())?;
    let file = workdir.join(out.trim());
    if let Some(dir) = file.parent() {
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| e.to_string())?;
    }
    let mut f = tokio::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .await
        .map_err(|e| e.to_string())?;
    f.write_all(b"\n.ratchet/\n")
        .await
        .map_err(|e| e.to_string())
}

/// `ratchet runner`: owns one agent on one branch.
pub async fn run_runner_loop(opts: RunnerLoopOptions) -> RunnerLoopResult {
    let RunnerLoopOptions {
        config,
        adapter,
        workdir,
        inbox,
        emit,
        clock,
        signal,
        tuning,
    } = opts;
    let remote = tuning
        .remote
        .clone()
        .unwrap_or_else(|| "origin".to_string());
    let best_branch = tuning
        .best_ref
        .clone()
        .unwrap_or_else(|| "ratchet/best".to_string());
    let best_tracking = tuning
        .best_tracking_ref
        .clone()
        .unwrap_or_else(|| format!("refs/remotes/{remote}/{best_branch}"));
    let accepted_ref = tuning
        .accepted_ref
        .clone()
        .unwrap_or_else(|| "refs/ratchet/accepted".to_string());
    let git = Git::new(
        &workdir,
        tuning.git.clone().unwrap_or_else(exec_git),
        tuning.git_env.clone(),
    );

    let (version, _) = watch::channel(0u64);
    let mailbox = Arc::new(Mailbox {
        state: Mutex::new(MailboxState::default()),
        version,
        cancel: CancellationToken::new(),
    });
    let core = Arc::new(Core {
        config,
        workdir,
        git,
        clock,
        emit_fn: emit,
        ctl: CancellationToken::new(),
        reason: Mutex::new(None),
        mailbox,
        shared: Mutex::new(Shared::default()),
        tuning,
        remote,
        best_branch,
        best_tracking,
        accepted_ref,
    });
    let mut local = Local {
        best: core.config.baseline_value,
        turns: 0,
        history: Vec::new(),
        best_sha: None,
    };

    // ---- control: one cancellation token for stop / deadline / external signal; `reason` says why.
    match &signal {
        Some(s) if s.is_cancelled() => core.halt(RunnerStopReason::Aborted),
        Some(s) => {
            let (core, s) = (core.clone(), s.clone());
            tokio::spawn(async move {
                tokio::select! {
                    _ = s.cancelled() => core.halt(RunnerStopReason::Aborted),
                    _ = core.ctl.cancelled() => {}
                }
            });
        }
        None => {}
    }
    {
        let core = core.clone();
        tokio::spawn(async move {
            while !core.ctl.is_cancelled() && core.clock.now() < core.config.deadline_at {
                let wait = (core.config.deadline_at - core.clock.now()).max(1) as u64;
                let _ = core.clock.sleep(wait, Some(&core.ctl)).await;
            }
            if !core.ctl.is_cancelled() {
                core.halt(RunnerStopReason::Deadline);
            }
        });
    }
    tokio::spawn(pump(core.clone(), inbox));

    if let Err(err) = body(&core, &mut local, adapter.as_ref()).await {
        core.emit(
            AgentEventType::Error,
            Some(format!("runner crashed: {err}")),
            Some(json!({"fatal": true})),
        );
        let mut reason = core.reason.lock().unwrap();
        reason.get_or_insert(RunnerStopReason::Failed);
    }

    let result = result_of(&core, &local);
    let state = match result.reason {
        RunnerStopReason::Failed | RunnerStopReason::AgentUnavailable => "failed",
        RunnerStopReason::Stopped | RunnerStopReason::Aborted => "stopped",
        _ => "done",
    };
    core.status(
        format!("runner finished: {}", result.reason),
        Some(json!({"state": state, "reason": result.reason.as_str(), "turns": result.turns, "candidates": result.candidates, "spendUsd": result.spend_usd})),
    );
    core.mailbox.close();
    core.ctl.cancel();
    result
}

fn result_of(core: &Core, local: &Local) -> RunnerLoopResult {
    RunnerLoopResult {
        reason: core.reason_or(RunnerStopReason::Stopped),
        turns: local.turns,
        candidates: local.history.len(),
        spend_usd: core.shared.lock().unwrap().spend_usd,
        best_value: local.best,
        history: local.history.clone(),
    }
}

async fn body(
    core: &Arc<Core>,
    local: &mut Local,
    adapter: &dyn AgentAdapter,
) -> Result<(), String> {
    let git = &core.git;
    let config = &core.config;
    let g = |e: crate::git::GitError| e.to_string();
    let max_idle = core.tuning.max_idle_turns.unwrap_or(2);
    let max_failures = core.tuning.max_consecutive_failures.unwrap_or(3);
    let retry_delay = core
        .tuning
        .retry_delay
        .unwrap_or(Duration::from_millis(5000));

    // ---- setup
    core.status(
        format!(
            "runner started for {} on {}",
            config.agent_id, config.branch
        ),
        Some(json!({"kind": adapter.kind().as_str(), "branch": config.branch})),
    );
    if git
        .try_run(&["config", "user.name"])
        .await
        .stdout
        .trim()
        .is_empty()
    {
        git.run(&[
            "config",
            "user.name",
            &format!("{} (ratchet)", config.agent_id),
        ])
        .await
        .map_err(g)?;
        git.run(&[
            "config",
            "user.email",
            &format!("{}@ratchet.local", config.agent_id),
        ])
        .await
        .map_err(g)?;
    }
    append_exclude(&core.workdir, git).await?;
    let base_head = git.head().await.map_err(g)?;
    git.run(&["update-ref", &core.accepted_ref, &base_head])
        .await
        .map_err(g)?;
    let mut known: HashSet<String> = HashSet::from([base_head]);

    let preflight = adapter.preflight().await.unwrap_or_else(|e| Preflight {
        ok: false,
        problems: vec![e.to_string()],
    });
    if !preflight.ok {
        core.emit(
            AgentEventType::Error,
            Some(format!(
                "agent preflight failed: {}",
                preflight.problems.join("; ")
            )),
            Some(json!({"problems": preflight.problems})),
        );
        core.set_reason(RunnerStopReason::AgentUnavailable);
        return Ok(());
    }

    let callback = {
        let core = core.clone();
        move |e: AgentEvent| core.on_agent_event(e)
    };

    // ---- the loop
    let mut session: Option<AgentSession> = None;
    let mut idle_turns = 0u32;
    let mut failures = 0u32;
    let mut notes: Vec<String> = Vec::new();
    let mut entry: Option<HistoryEntry> = None;
    let mut no_change = false;

    while !core.ctl.is_cancelled() {
        core.wait_unpaused().await;
        if core.ctl.is_cancelled() {
            break;
        }
        local.turns += 1;
        let context_text = core.write_context(local, local.turns).await?;
        let spec = AgentSpec {
            workdir: core.workdir.to_string_lossy().into_owned(),
            prompt: String::new(),
            model: core.tuning.model.clone().filter(|m| !m.is_empty()),
            max_turns: core.tuning.max_turns.filter(|t| *t > 0),
            env: core.tuning.agent_env.clone(),
        };

        // -- agent turn
        let turn_result: PortResult<()> = if let Some(s) = session.as_mut() {
            core.status(
                "agent turn started",
                Some(json!({"turn": local.turns, "resumed": true})),
            );
            let spend = core.shared.lock().unwrap().spend_usd;
            let user_messages: Vec<String> =
                std::mem::take(&mut core.mailbox.state.lock().unwrap().user_messages);
            let message = build_resume_message(&ResumeInput {
                entry: entry.as_ref(),
                best_value: local.best,
                config,
                now: core.clock.now(),
                spend_usd: spend,
                notes: &notes,
                user_messages: &user_messages,
                no_change,
            });
            adapter
                .resume(s, &spec, &message, &callback, Some(&core.ctl))
                .await
        } else {
            core.status(
                "agent turn started",
                Some(json!({"turn": local.turns, "resumed": false})),
            );
            let start_spec = AgentSpec {
                prompt: build_initial_prompt(config, &context_text),
                ..spec
            };
            match adapter.start(&start_spec, &callback, Some(&core.ctl)).await {
                Ok(s) => {
                    session = Some(s);
                    Ok(())
                }
                Err(e) => Err(e),
            }
        };
        match turn_result {
            Ok(()) => failures = 0,
            Err(err) => {
                if err.is_cancelled() || core.ctl.is_cancelled() {
                    break;
                }
                failures += 1;
                let text = describe(&err);
                core.emit(
                    AgentEventType::Error,
                    Some(format!("agent turn failed: {text}")),
                    Some(json!({"failures": failures})),
                );
                if failures >= max_failures {
                    core.set_reason(RunnerStopReason::Failed);
                    break;
                }
                let _ = core
                    .clock
                    .sleep(retry_delay.as_millis() as u64, Some(&core.ctl))
                    .await;
                // retry as a resume if we have a session, else restart; tell the agent what happened
                notes = vec![format!(
                    "Your previous turn crashed ({text}). Check the tree for half-finished edits and continue."
                )];
                entry = None;
                no_change = false;
                if session.is_none() {
                    local.turns -= 1;
                }
                continue;
            }
        }

        // -- submit
        let candidate = core.commit_if_changed(local.turns, &known).await?;
        entry = None;
        no_change = false;
        let Some(candidate) = candidate else {
            idle_turns += 1;
            core.status(
                "no change after agent turn",
                Some(json!({"idleTurns": idle_turns})),
            );
            if idle_turns >= max_idle && core.mailbox.state.lock().unwrap().user_messages.is_empty()
            {
                core.set_reason(RunnerStopReason::Idle);
                break;
            }
            no_change = true;
            notes = core.sync_to_best(local).await?;
            known.insert(git.head().await.map_err(g)?);
            continue;
        };
        idle_turns = 0;
        if !core.push().await {
            core.reason
                .lock()
                .unwrap()
                .get_or_insert(RunnerStopReason::Failed);
            break;
        }
        known.insert(candidate.sha.clone());
        core.emit(
            AgentEventType::Commit,
            Some(candidate.subject.clone()),
            Some(json!({"sha": candidate.sha, "branch": config.branch, "files": candidate.files})),
        );

        // -- wait for the verdict
        core.status("waiting for verdict", Some(json!({"sha": candidate.sha})));
        loop {
            let mut rx = core.mailbox.subscribe();
            if core
                .mailbox
                .state
                .lock()
                .unwrap()
                .verdicts
                .contains_key(&candidate.sha)
                || core.ctl.is_cancelled()
            {
                break;
            }
            Mailbox::changed(&mut rx, &core.ctl).await;
        }
        let Some(verdict) = core
            .mailbox
            .state
            .lock()
            .unwrap()
            .verdicts
            .remove(&candidate.sha)
        else {
            break;
        };
        local.best = verdict.current_best;
        let e = HistoryEntry {
            n: local.history.len() + 1,
            sha: candidate.sha.clone(),
            verdict: verdict.verdict,
            reasons: verdict.reasons.clone(),
            best: verdict.current_best,
            at: core.clock.now(),
        };
        local.history.push(e.clone());
        entry = Some(e);
        core.emit(
            AgentEventType::Verdict,
            Some(verdict.verdict.to_string()),
            Some(json!({"sha": candidate.sha, "verdict": verdict.verdict.as_str(), "reasons": verdict.reasons, "currentBest": verdict.current_best})),
        );
        if verdict.verdict == CandidateVerdict::Verified {
            git.run(&["update-ref", &core.accepted_ref, &candidate.sha])
                .await
                .map_err(g)?;
            core.mailbox.state.lock().unwrap().latest_best = Some(LatestBest {
                sha: candidate.sha.clone(),
                branch: config.branch.clone(),
            });
        }

        notes = core.sync_to_best(local).await?;
        known.insert(git.head().await.map_err(g)?);
        if core.shared.lock().unwrap().over_budget {
            core.set_reason(RunnerStopReason::Budget);
            break;
        }
    }
    Ok(())
}
