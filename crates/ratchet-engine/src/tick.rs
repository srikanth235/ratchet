//! The control loop: `tick`, runner events, candidates, the bench queue, the ratchet verdicts. Part of the port of
//! `packages/engine/src/engine.ts`.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use ratchet_core::enums::{
    AgentStatus, BenchPhase, CandidateVerdict, OptimizationStatus, Verdict,
};
use ratchet_core::numfmt::{js_number_to_string, to_fixed};
use ratchet_core::ports::{
    AgentEvent, AgentEventType, BaselineMetrics, BenchJob, BenchJobPhase, BenchOutcome, CancellationToken,
    EngineEvent, NoticeLevel, PolledEvents, PortError, RunnerMessage,
};
use ratchet_core::rows::{Agent, CandidateWithResults, Optimization};
use ratchet_core::{
    ProtectedConfigInput, RatchetInput, RatchetOutcome, effective_protected,
    evaluate_ratchet, find_protected_violations, median,
};
use ratchet_ledger::{
    AppendAgentEventInput, LedgerErrorCode, Patch, RecordBenchResultInput, RecordSpendInput,
    SubmitCandidateInput,
};
use serde_json::json;
use tokio::sync::watch;
use tokio::task::JoinHandle;

use crate::bench_stats::{metrics_from_stats, samples_of, stats_of};
use crate::deps::token_cost_usd;
use crate::engine::{
    Engine, ResumeReport, aid, event_type_str, handle, live_agent_status, lock, oid,
    terminal_agent_status,
};
use crate::errors::{EngineError, EngineResult};
use crate::kv::k;
use crate::reaper::reap;

pub(crate) struct JobState {
    pub candidate_id: String,
    pub phase: BenchPhase,
    pub result: Mutex<Option<Result<BenchOutcome, PortError>>>,
    pub handle: Mutex<Option<JoinHandle<()>>>,
}

/// Per-optimization state that lives only in this process.
pub struct Runtime {
    /// Set synchronously when a kill switch starts, so a concurrent tick bails out at its next checkpoint.
    pub killing: AtomicBool,
    pub abort: CancellationToken,
    job: Mutex<Option<Arc<JobState>>>,
    attempts: Mutex<HashMap<String, u32>>,
    poll_failures: Mutex<HashMap<String, u32>>,
    last_served_agent: Mutex<Option<String>>,
    last_spend_emitted: Mutex<Option<f64>>,
}

impl Runtime {
    pub(crate) fn new() -> Self {
        Self {
            killing: AtomicBool::new(false),
            abort: CancellationToken::new(),
            job: Mutex::new(None),
            attempts: Mutex::new(HashMap::new()),
            poll_failures: Mutex::new(HashMap::new()),
            last_served_agent: Mutex::new(None),
            last_spend_emitted: Mutex::new(None),
        }
    }

    pub(crate) fn clear_job(&self) {
        *lock(&self.job) = None;
    }

    fn job(&self) -> Option<Arc<JobState>> {
        lock(&self.job).clone()
    }
}

#[doc(hidden)]
pub struct RunLoop {
    pub cancel: CancellationToken,
    pub done: watch::Receiver<bool>,
}

fn status_list() -> [OptimizationStatus; 3] {
    [
        OptimizationStatus::Baselining,
        OptimizationStatus::Running,
        OptimizationStatus::Stopping,
    ]
}

impl Engine {
    // ------------------------------------------------------------------ tick / run

    /// One pass of the control loop over every optimization. Safe to call concurrently (calls queue).
    pub async fn tick(&self) -> EngineResult<()> {
        let _turn = self.tick_lock.lock().await;
        self.tick_once().await
    }

    /// Await bench jobs that are in flight (tests and shutdown). Does not tick.
    pub async fn settle(&self) {
        let jobs: Vec<Arc<JobState>> = lock(&self.rt).values().filter_map(|r| r.job()).collect();
        for j in jobs {
            let h = lock(&j.handle).take();
            if let Some(h) = h {
                let _ = h.await;
            }
        }
    }

    /// True while any bench job is running.
    pub fn busy(&self) -> bool {
        lock(&self.rt)
            .values()
            .any(|r| r.job().is_some_and(|j| lock(&j.result).is_none()))
    }

    /// Tick every `tick_interval_ms` (via the injected clock) until `cancel` fires or `close()` is called.
    pub async fn run(&self, cancel: Option<CancellationToken>) {
        let token = match cancel {
            Some(c) => c.child_token(),
            None => CancellationToken::new(),
        };
        let (done_tx, done_rx) = watch::channel(false);
        {
            let mut g = lock(&self.run_loop);
            if let Some(existing) = g.as_ref() {
                let mut rx = existing.done.clone();
                drop(g);
                while !*rx.borrow() {
                    if rx.changed().await.is_err() {
                        break;
                    }
                }
                return;
            }
            *g = Some(RunLoop {
                cancel: token.clone(),
                done: done_rx,
            });
        }
        while !token.is_cancelled() {
            if let Err(e) = self.tick().await {
                self.report_error(e, "tick");
            }
            if token.is_cancelled() {
                break;
            }
            let _ = self
                .clock
                .sleep(self.settings.tick_interval_ms, Some(&token))
                .await;
            tokio::task::yield_now().await;
        }
        *lock(&self.run_loop) = None;
        let _ = done_tx.send(true);
    }

    /// Stop the `run()` loop and abort in-flight bench work. Does not touch cloud resources.
    pub async fn close(&self) {
        let rl = lock(&self.run_loop)
            .as_ref()
            .map(|r| (r.cancel.clone(), r.done.clone()));
        if let Some((cancel, mut done)) = rl {
            cancel.cancel();
            while !*done.borrow() {
                if done.changed().await.is_err() {
                    break;
                }
            }
        }
        let rts: Vec<Arc<Runtime>> = lock(&self.rt).values().cloned().collect();
        for r in rts {
            r.abort.cancel();
        }
        self.settle().await;
    }

    async fn tick_once(&self) -> EngineResult<()> {
        self.ready().await;
        for summary in self.ledger.list_optimizations(None).await? {
            let id = summary.id.to_string();
            let r = match summary.status {
                OptimizationStatus::Baselining => async {
                    self.enforce_limits(&id).await?;
                    self.reap_one(&id).await
                }
                .await,
                OptimizationStatus::Running => self.tick_running(&id).await,
                OptimizationStatus::Stopping => self.tick_stopping(&id).await,
                OptimizationStatus::Stopped
                | OptimizationStatus::Completed
                | OptimizationStatus::Failed => self.tick_ended(&id).await,
                OptimizationStatus::Draft => Ok(()),
            };
            if let Err(e) = r {
                self.report_error(e, &format!("tick {id}"));
            }
        }
        // After the per-run work: the idle clock of the automatic sandbox (destroys it once no run has been active for
        // the idle window).
        self.auto_tick().await;
        self.sync_run_active().await;
        Ok(())
    }

    async fn tick_running(&self, id: &str) -> EngineResult<()> {
        let rt = self.state(id);
        if rt.killing.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.collect_bench_result(id).await?;
        let opt = self.require_opt(id).await?;
        let agents = self.ledger.list_agents(&oid(id)).await?;
        // An agent that reports done / failed in this very poll may have pushed its last commit just before: it still
        // gets one more git poll in this tick (agents live at the start of the tick are polled).
        let mut live_at_start: HashSet<String> = HashSet::new();
        for a in &agents {
            if live_agent_status(self.require_agent(a.id.as_str()).await?.status) {
                live_at_start.insert(a.id.to_string());
            }
        }
        for a in &agents {
            if rt.killing.load(Ordering::SeqCst) {
                return Ok(());
            }
            let cur = self.require_agent(a.id.as_str()).await?;
            self.poll_runner(&opt, &cur, false).await?;
        }
        for a in &agents {
            if rt.killing.load(Ordering::SeqCst) {
                return Ok(());
            }
            let cur = self.require_agent(a.id.as_str()).await?;
            if live_agent_status(cur.status) || live_at_start.contains(a.id.as_str()) {
                self.poll_git(&opt, &cur).await?;
            }
        }
        if rt.killing.load(Ordering::SeqCst) {
            return Ok(());
        }
        if self.enforce_limits(id).await? {
            return Ok(());
        }
        self.schedule_bench(id).await?;
        self.reap_one(id).await
    }

    /// Graceful stop in progress: keep evaluating queued candidates, then finish.
    async fn tick_stopping(&self, id: &str) -> EngineResult<()> {
        let rt = self.state(id);
        if rt.killing.load(Ordering::SeqCst)
            || self.kv.get(&k::stop_mode(id)).await?.as_deref() != Some("graceful")
        {
            return Ok(());
        }
        self.collect_bench_result(id).await?;
        if self.enforce_limits(id).await? {
            return Ok(());
        }
        let since = self
            .kv
            .get_i64(&k::stop_at(id))
            .await?
            .unwrap_or_else(|| self.clock.now());
        if self.clock.now() - since > self.settings.drain_timeout_ms {
            let reason = self
                .kv
                .get(&k::stop_reason(id))
                .await?
                .unwrap_or_else(|| "stopped".into());
            self.execute_kill(
                id,
                &format!("{reason} (drain timeout)"),
                OptimizationStatus::Stopped,
            )
            .await?;
            return Ok(());
        }
        // Keep collecting while draining: a failed or lagging git listing must not strand a candidate.
        let opt = self.require_opt(id).await?;
        let git_complete = self.drain_poll(&opt).await?;
        if rt.killing.load(Ordering::SeqCst) {
            return Ok(());
        }
        self.schedule_bench(id).await?;
        let pending = self
            .ledger
            .candidates_with_results(&oid(id))
            .await?
            .iter()
            .any(|c| c.candidate.verdict == CandidateVerdict::Pending);
        if git_complete && rt.job().is_none() && !pending {
            let reason = self
                .kv
                .get(&k::stop_reason(id))
                .await?
                .unwrap_or_else(|| "stopped".into());
            self.execute_kill(id, &reason, OptimizationStatus::Stopped)
                .await?;
            return Ok(());
        }
        self.reap_one(id).await
    }

    /// Stopped / failed / completed optimizations: keep the reaper and any failed revocation retrying.
    async fn tick_ended(&self, id: &str) -> EngineResult<()> {
        for a in self.ledger.list_agents(&oid(id)).await? {
            if self.kv.has(&k::identity(a.id.as_str())).await? {
                self.revoke_agent(&a).await;
            }
        }
        self.reap_one(id).await
    }

    async fn reap_one(&self, id: &str) -> EngineResult<()> {
        let instances = self.ports.compute.list_tagged(Some(&oid(id))).await?;
        let killed = reap(&self.deps, id, Some(instances)).await?;
        if !killed.is_empty() {
            self.safe_meter(id).await;
        }
        Ok(())
    }

    // ------------------------------------------------------------------ graceful stop

    /// Time limit / plateau / all agents done: stop the agents cleanly and let queued candidates finish.
    async fn begin_graceful(&self, id: &str, reason: &str) -> EngineResult<()> {
        let opt = self.require_opt(id).await?;
        if opt.status != OptimizationStatus::Running {
            return Ok(());
        }
        self.kv.set(&k::stop_mode(id), "graceful").await?;
        self.kv.set(&k::stop_reason(id), reason).await?;
        self.kv
            .set(&k::stop_at(id), &self.clock.now().to_string())
            .await?;
        self.set_status(id, OptimizationStatus::Stopping).await?;
        let agents = self.ledger.list_agents(&oid(id)).await?;
        let runners = self.runners_for(id).await;
        for a in agents
            .iter()
            .filter(|a| a.runner_instance_id.is_some() && live_agent_status(a.status))
        {
            if let Err(e) = runners.stop(&handle(a)).await {
                self.report_error(e, &format!("runners.stop {}", a.id));
            }
        }
        // The runners are down: collect what they left behind (events first, then a final git poll of every branch),
        // then close the agents' access. A git poll that fails here is retried on every drain tick.
        self.drain_poll(&opt).await?;
        for a in self.ledger.list_agents(&oid(id)).await? {
            self.revoke_agent(&a).await;
        }
        Ok(())
    }

    /// One collection pass while stopping: every runner's last events, then every agent branch, so a commit pushed just
    /// before (or while) the runner reported done is never missed. Agents still marked live are closed afterwards.
    /// Returns true only when every branch was listed successfully.
    async fn drain_poll(&self, opt: &Optimization) -> EngineResult<bool> {
        let mut complete = true;
        for a in self.ledger.list_agents(&opt.id).await? {
            let cur = self.require_agent(a.id.as_str()).await?;
            self.poll_runner(opt, &cur, true).await?;
            let cur = self.require_agent(a.id.as_str()).await?;
            if !self.poll_git(opt, &cur).await? {
                complete = false;
            }
            let after = self.require_agent(a.id.as_str()).await?;
            if !terminal_agent_status(after.status) {
                self.ledger
                    .set_agent_status(&a.id, AgentStatus::Stopped, Patch::Keep)
                    .await?;
            }
        }
        Ok(complete)
    }

    // ------------------------------------------------------------------ cost + stop rules

    /// Meter spend, then apply budget / deadline / plateau rules. Returns true when it started a stop.
    async fn enforce_limits(&self, id: &str) -> EngineResult<bool> {
        let rt = self.state(id);
        let opt = self.require_opt(id).await?;
        self.safe_meter(id).await;
        let total = self.ledger.total_spend(&oid(id)).await?;
        let changed = {
            let mut last = lock(&rt.last_spend_emitted);
            let changed = *last != Some(total);
            *last = Some(total);
            changed
        };
        if changed {
            self.emit(EngineEvent::Spend {
                optimization_id: oid(id),
                total_usd: total,
            });
        }
        if total >= opt.budget_usd {
            self.execute_kill(
                id,
                &format!(
                    "budget exhausted (${} of ${})",
                    to_fixed(total, 2),
                    to_fixed(opt.budget_usd, 2)
                ),
                OptimizationStatus::Stopped,
            )
            .await?;
            return Ok(true);
        }
        if total >= opt.budget_usd * self.settings.warn_fraction
            && !self.kv.has(&k::warned(id)).await?
        {
            self.kv
                .set(&k::warned(id), &self.clock.now().to_string())
                .await?;
            self.emit(EngineEvent::Notice {
                optimization_id: oid(id),
                level: NoticeLevel::Warning,
                text: format!(
                    "spend ${} reached {}% of the ${} budget",
                    to_fixed(total, 2),
                    (self.settings.warn_fraction * 100.0).round(),
                    to_fixed(opt.budget_usd, 2)
                ),
            });
        }
        if opt.status != OptimizationStatus::Running {
            return Ok(false);
        }

        if let Some(deadline) = self.kv.get_i64(&k::deadline(id)).await?
            && self.clock.now() >= deadline
        {
            self.begin_graceful(id, "time limit reached").await?;
            return Ok(true);
        }
        if let Some(plateau) = self.kv.get_i64(&k::plateau(id)).await? {
            let streak = self.no_gain_streak(id).await?;
            if streak as i64 >= plateau {
                self.begin_graceful(
                    id,
                    &format!("plateau: {streak} consecutive evaluations without a verified win"),
                )
                .await?;
                return Ok(true);
            }
        }
        let agents = self.ledger.list_agents(&oid(id)).await?;
        if !agents.is_empty() && agents.iter().all(|a| terminal_agent_status(a.status)) {
            self.begin_graceful(id, "all agents finished").await?;
            return Ok(true);
        }
        Ok(false)
    }

    /// Trailing run of decided, non-verified candidates (blocked, rejected and failed-confirm all count).
    async fn no_gain_streak(&self, id: &str) -> EngineResult<usize> {
        let mut decided: Vec<_> = self
            .ledger
            .candidates_with_results(&oid(id))
            .await?
            .into_iter()
            .filter(|c| c.candidate.verdict != CandidateVerdict::Pending)
            .collect();
        decided.sort_by_key(|c| (c.candidate.verdict_at.unwrap_or(0), c.candidate.submitted_at));
        Ok(decided
            .iter()
            .rev()
            .take_while(|c| c.candidate.verdict != CandidateVerdict::Verified)
            .count())
    }

    // ------------------------------------------------------------------ runner events

    async fn poll_runner(&self, opt: &Optimization, agent: &Agent, force: bool) -> EngineResult<()> {
        if agent.runner_instance_id.is_none() {
            return Ok(());
        }
        if !force && !live_agent_status(agent.status) {
            return Ok(());
        }
        let rt = self.state(opt.id.as_str());
        let cursor = self.kv.get(&k::cursor(agent.id.as_str())).await?;
        let runners = self.runners_for(agent.optimization_id.as_str()).await;
        let res: PolledEvents = match runners.poll(&handle(agent), cursor.as_deref()).await {
            Ok(r) => r,
            Err(err) => {
                let n = {
                    let mut f = lock(&rt.poll_failures);
                    let n = f.entry(agent.id.to_string()).or_default();
                    *n += 1;
                    *n
                };
                if n == 1 {
                    self.report_error(err, &format!("poll {}", agent.id));
                }
                return Ok(());
            }
        };
        lock(&rt.poll_failures).remove(agent.id.as_str());
        if res.events.is_empty() && Some(res.cursor.as_str()) == cursor.as_deref() {
            return Ok(());
        }

        let token_usd: f64 = res
            .events
            .iter()
            .filter_map(|e| e.usage.as_ref())
            .map(|u| token_cost_usd(u, &self.settings.token_pricing))
            .sum();
        let now = self.clock.now();
        let new_status = status_from_events(&res.events);
        // Events, token spend and the cursor commit together: a crash can neither lose nor replay events.
        let (events, aid_, opt_id, cursor_key, new_cursor) = (
            res.events.clone(),
            agent.id.clone(),
            opt.id.clone(),
            k::cursor(agent.id.as_str()),
            res.cursor.clone(),
        );
        let current_status = agent.status;
        self.ledger
            .atomically(move |mut tx| {
                Box::pin(async move {
                    for e in &events {
                        tx.append_agent_event(AppendAgentEventInput {
                            agent_id: aid_.clone(),
                            event_type: event_type_str(e.event_type),
                            payload: Some(serde_json::to_value(e).map_err(|x| {
                                ratchet_core::LedgerError::invalid(x.to_string())
                            })?),
                            at: Some(e.at),
                        })
                        .await?;
                    }
                    if token_usd > 0.0 {
                        let mut s = RecordSpendInput::new(
                            opt_id.clone(),
                            ratchet_core::enums::SpendCategory::Tokens,
                            token_usd,
                            format!("tokens:{aid_}"),
                        );
                        s.agent_id = Some(aid_.clone());
                        s.estimated = Some(true);
                        s.at = Some(now);
                        tx.record_spend(s).await?;
                    }
                    tx.kv_set(&cursor_key, &new_cursor).await?;
                    if let Some(st) = new_status
                        && st != current_status
                    {
                        tx.set_agent_status(&aid_, st).await?;
                    }
                    Ok(())
                })
            })
            .await?;
        for e in res.events {
            self.emit(EngineEvent::AgentEvent {
                optimization_id: opt.id.clone(),
                agent_id: agent.id.clone(),
                event: e,
            });
        }
        if token_usd > 0.0 {
            self.emit(EngineEvent::Spend {
                optimization_id: opt.id.clone(),
                total_usd: self.ledger.total_spend(&opt.id).await?,
            });
        }
        Ok(())
    }

    // ------------------------------------------------------------------ git: candidates

    /// Submit the agent branch's new candidate commits. False when the branch could not be fully read (retry later).
    async fn poll_git(&self, opt: &Optimization, agent: &Agent) -> EngineResult<bool> {
        let config = self.config(opt.id.as_str()).await?;
        let git = self.git_for(&opt.repo);
        let base_sha = match self.base_sha(opt).await {
            Ok(s) => s,
            Err(err) => {
                self.report_error(err, &format!("resolveRef {}", opt.base_ref));
                opt.base_ref.clone()
            }
        };
        let all = self.ledger.candidates_with_results(&opt.id).await?;
        let mut seq = all.len();
        let mine: Vec<&CandidateWithResults> = all
            .iter()
            .filter(|c| c.candidate.agent_id == agent.id)
            .collect();
        let mut known: HashSet<String> = mine.iter().map(|c| c.candidate.sha.clone()).collect();
        let last = mine.last().map(|c| c.candidate.sha.clone());
        let found = match git.list_candidates(&agent.branch, last.as_deref()).await {
            Ok(f) => f,
            Err(err) => {
                self.report_error(err, &format!("listCandidates {}", agent.branch));
                return Ok(false);
            }
        };
        let globs = effective_protected(&ProtectedConfigInput::from(&*config), None).globs;
        for c in found {
            if known.contains(&c.sha) {
                continue;
            }
            let files = match git.changed_files(&base_sha, &c.sha).await {
                Ok(f) => f,
                Err(err) => {
                    self.report_error(err, &format!("changedFiles {}", c.sha));
                    return Ok(false); // keep order: retry this commit (and the rest) next tick
                }
            };
            let violations = find_protected_violations(&files, &globs);
            let now = self.clock.now();
            // Sortable ids: the ledger orders by (submitted_at, id), and one poll can find several commits in the same
            // millisecond.
            seq += 1;
            let candidate_id = format!("{}-c{seq:05}", opt.id);
            let mut input = SubmitCandidateInput::new(opt.id.clone(), agent.id.clone(), c.sha.clone());
            input.id = Some(candidate_id.clone().into());
            input.message = Some(c.message.clone());
            input.submitted_at = Some(now);
            let row = self.ledger.submit_candidate(input).await?;
            known.insert(c.sha.clone());
            self.emit(EngineEvent::Candidate {
                optimization_id: opt.id.clone(),
                candidate_id: row.id.clone(),
                verdict: CandidateVerdict::Pending,
            });
            if !violations.is_empty() {
                let reasons = violations
                    .iter()
                    .map(|v| format!("protected path changed: {} (matches {})", v.path, v.pattern))
                    .collect();
                self.finalize(opt, row.id.as_str(), agent.id.as_str(), &row.sha, Verdict::Blocked, reasons, None)
                    .await?;
            }
        }
        Ok(true)
    }

    // ------------------------------------------------------------------ bench queue

    async fn schedule_bench(&self, id: &str) -> EngineResult<()> {
        let rt = self.state(id);
        if rt.job().is_some() || rt.killing.load(Ordering::SeqCst) {
            return Ok(());
        }
        let opt = self.require_opt(id).await?;
        let config = self.config(id).await?;

        // Recover candidates whose bench result is already recorded (after a crash, or a confirm that is due).
        let mut needs_confirm: Option<CandidateWithResults> = None;
        for c in self.ledger.candidates_with_results(&oid(id)).await? {
            if c.candidate.verdict != CandidateVerdict::Pending || c.results.is_empty() {
                continue;
            }
            match self.settle_candidate(&opt, &c).await {
                Ok(outcome) => {
                    if outcome.is_some_and(|o| o.verdict == Verdict::PendingConfirm)
                        && needs_confirm.is_none()
                    {
                        needs_confirm = Some(c);
                    }
                }
                Err(err) => {
                    // Never let one unevaluable candidate wedge the queue (and the drain) forever.
                    self.report_error(&err, &format!("settle {}", c.candidate.sha));
                    self.finalize(
                        &opt,
                        c.candidate.id.as_str(),
                        c.candidate.agent_id.as_str(),
                        &c.candidate.sha,
                        Verdict::Rejected,
                        vec![format!("could not evaluate candidate: {}", err.message)],
                        None,
                    )
                    .await?;
                }
            }
        }
        if let Some(c) = needs_confirm {
            return self.launch_job(&opt, &config, &c, BenchPhase::Confirm).await;
        }

        let pending: Vec<CandidateWithResults> = self
            .ledger
            .candidates_with_results(&oid(id))
            .await?
            .into_iter()
            .filter(|c| c.candidate.verdict == CandidateVerdict::Pending && c.results.is_empty())
            .collect();
        if pending.is_empty() {
            return Ok(());
        }
        let agents = self.ledger.list_agents(&oid(id)).await?;
        let last = lock(&rt.last_served_agent).clone();
        let start = last
            .and_then(|l| agents.iter().position(|a| a.id.as_str() == l))
            .map_or(0, |p| p + 1);
        for i in 0..agents.len() {
            let agent = &agents[(start + i) % agents.len()];
            if let Some(next) = pending.iter().find(|c| c.candidate.agent_id == agent.id) {
                *lock(&rt.last_served_agent) = Some(agent.id.to_string());
                return self.launch_job(&opt, &config, next, BenchPhase::Bench).await;
            }
        }
        Ok(())
    }

    async fn launch_job(
        &self,
        opt: &Optimization,
        config: &Arc<ratchet_core::config::RatchetConfig>,
        cand: &CandidateWithResults,
        phase: BenchPhase,
    ) -> EngineResult<()> {
        let rt = self.state(opt.id.as_str());
        let baseline = self.ledger.get_baseline(&opt.id).await?;
        let job = BenchJob {
            optimization_id: opt.id.clone(),
            candidate_id: Some(cand.candidate.id.clone()),
            sha: cand.candidate.sha.clone(),
            phase: match phase {
                BenchPhase::Bench => BenchJobPhase::Bench,
                BenchPhase::Confirm => BenchJobPhase::Confirm,
            },
            repeats: self.repeats_for(opt.id.as_str(), config).await?,
            trace: if phase == BenchPhase::Confirm {
                config.evaluator.confirm_trace.clone()
            } else {
                None
            },
        };
        let baseline_metrics = baseline.map(|b| BaselineMetrics {
            metrics: metrics_from_stats(&b.stats),
        });
        let state = Arc::new(JobState {
            candidate_id: cand.candidate.id.to_string(),
            phase,
            result: Mutex::new(None),
            handle: Mutex::new(None),
        });
        let (bench, config, cancel, st) = (
            self.ports.bench.clone(),
            config.clone(),
            rt.abort.clone(),
            state.clone(),
        );
        let task = tokio::spawn(async move {
            let r = bench
                .run(&job, &config, baseline_metrics.as_ref(), Some(&cancel))
                .await;
            *lock(&st.result) = Some(r);
        });
        *lock(&state.handle) = Some(task);
        *lock(&rt.job) = Some(state);
        self.log_candidate(
            cand.candidate.agent_id.as_str(),
            &cand.candidate.sha,
            &format!("{phase} started"),
            AgentEventType::Status,
        )
        .await
    }

    async fn collect_bench_result(&self, id: &str) -> EngineResult<()> {
        let rt = self.state(id);
        let Some(job) = rt.job() else {
            return Ok(());
        };
        let Some(result) = lock(&job.result).clone() else {
            return Ok(());
        };
        rt.clear_job();
        if rt.killing.load(Ordering::SeqCst) {
            return Ok(());
        }
        let opt = self.require_opt(id).await?;
        let Some(cand) = self
            .ledger
            .candidates_with_results(&oid(id))
            .await?
            .into_iter()
            .find(|c| c.candidate.id.as_str() == job.candidate_id)
        else {
            return Ok(());
        };
        if cand.candidate.verdict != CandidateVerdict::Pending {
            return Ok(());
        }

        let outcome = match result {
            Err(err) => {
                return self
                    .bench_attempt_failed(&rt, &opt, &cand, job.phase, err.to_string())
                    .await; // otherwise the scheduler retries it next tick
            }
            Ok(o) => o,
        };
        // A guardrail that fails before any bench run leaves no samples (the pipeline fails fast): the candidate is
        // blocked outright, there is nothing to record or to retry.
        if outcome.samples.is_empty() && outcome.runs.is_empty() {
            let failed: Vec<_> = outcome.guardrails.iter().filter(|g| !g.passed).collect();
            if !failed.is_empty() {
                let reasons = failed
                    .iter()
                    .map(|g| match &g.detail {
                        Some(d) if !d.is_empty() => format!("guardrail failed: {} ({d})", g.name),
                        _ => format!("guardrail failed: {}", g.name),
                    })
                    .collect();
                return self
                    .finalize(
                        &opt,
                        cand.candidate.id.as_str(),
                        cand.candidate.agent_id.as_str(),
                        &cand.candidate.sha,
                        Verdict::Blocked,
                        reasons,
                        None,
                    )
                    .await;
            }
        }
        let recorded: EngineResult<()> = async {
            let samples = samples_of(&outcome).map_err(EngineError::internal)?;
            let stats = stats_of(&outcome).map_err(EngineError::internal)?;
            self.ledger
                .record_bench_result(RecordBenchResultInput {
                    candidate_id: cand.candidate.id.clone(),
                    phase: job.phase,
                    samples,
                    stats,
                    guardrails: outcome.guardrails.clone(),
                    started_at: outcome.started_at,
                    finished_at: outcome.finished_at,
                })
                .await?;
            Ok(())
        }
        .await;
        if let Err(err) = recorded {
            // An unusable outcome counts like a failed run, so it can never be retried forever.
            return self
                .bench_attempt_failed(&rt, &opt, &cand, job.phase, err.message)
                .await;
        }
        let fresh = self.require_candidate(id, cand.candidate.id.as_str()).await?;
        self.settle_candidate(&opt, &fresh).await?;
        Ok(())
    }

    /// Count a failed bench attempt; after `max_bench_attempts` the candidate is rejected as an infrastructure failure.
    async fn bench_attempt_failed(
        &self,
        rt: &Arc<Runtime>,
        opt: &Optimization,
        cand: &CandidateWithResults,
        phase: BenchPhase,
        msg: String,
    ) -> EngineResult<()> {
        let attempts = {
            let mut a = lock(&rt.attempts);
            let n = a.entry(format!("{}:{phase}", cand.candidate.id)).or_default();
            *n += 1;
            *n
        };
        self.report_error(&msg, &format!("bench {phase} {}", cand.candidate.sha));
        self.log_candidate(
            cand.candidate.agent_id.as_str(),
            &cand.candidate.sha,
            &format!("{phase} failed (attempt {attempts}): {msg}"),
            AgentEventType::Error,
        )
        .await?;
        if attempts >= self.settings.max_bench_attempts {
            self.finalize(
                opt,
                cand.candidate.id.as_str(),
                cand.candidate.agent_id.as_str(),
                &cand.candidate.sha,
                Verdict::Rejected,
                vec![format!(
                    "bench infrastructure error after {attempts} attempts: {msg}"
                )],
                None,
            )
            .await?;
        }
        Ok(())
    }

    /// Evaluate a candidate purely from what the ledger holds (bench result, optional confirm result, baseline noise
    /// floor, current best) and finalize it unless it still needs its confirm run. Because it only reads the ledger, it
    /// is also how a restarted engine finishes interrupted work.
    async fn settle_candidate(
        &self,
        opt: &Optimization,
        cand: &CandidateWithResults,
    ) -> EngineResult<Option<RatchetOutcome>> {
        let Some(bench) = cand.results.iter().rev().find(|r| r.phase == BenchPhase::Bench) else {
            return Ok(None);
        };
        let confirm = cand.results.iter().rev().find(|r| r.phase == BenchPhase::Confirm);
        let baseline = self.ledger.get_baseline(&opt.id).await?;
        let current_best = self.ledger.current_best(&opt.id).await?;
        let (Some(baseline), Some(current_best)) = (baseline, current_best) else {
            return Ok(None);
        };
        let confirm_best = self.kv.get_number(&k::confirm_best(opt.id.as_str())).await?;
        let config = self.config(opt.id.as_str()).await?;
        let min_gain = self
            .calibration_for(opt.id.as_str(), &config)
            .await?
            .map_or(opt.min_gain, |c| c.min_gain);
        let mut guardrails = bench.guardrails.clone();
        if let Some(c) = confirm {
            guardrails.extend(c.guardrails.iter().cloned());
        }
        let outcome = evaluate_ratchet(&RatchetInput {
            direction: opt.direction,
            min_gain,
            noise_floor: baseline.noise_floor,
            current_best,
            candidate: bench.samples.clone(),
            confirm: confirm.map(|c| c.samples.clone()),
            confirm_best,
            guardrails,
            protected_violations: vec![],
        })
        .map_err(|e| EngineError::internal(e.to_string()))?;
        if outcome.verdict == Verdict::PendingConfirm {
            return Ok(Some(outcome));
        }
        self.finalize(
            opt,
            cand.candidate.id.as_str(),
            cand.candidate.agent_id.as_str(),
            &cand.candidate.sha,
            outcome.verdict,
            outcome.reasons.clone(),
            Some((outcome.clone(), confirm.map(|c| c.samples.clone()))),
        )
        .await?;
        Ok(Some(outcome))
    }

    /// Persist the verdict (and the ratchet move) atomically, then tell the world and the runners.
    #[allow(clippy::too_many_arguments)]
    async fn finalize(
        &self,
        opt: &Optimization,
        cand_id: &str,
        agent_id: &str,
        sha: &str,
        verdict: Verdict,
        reasons: Vec<String>,
        detail: Option<(RatchetOutcome, Option<Vec<f64>>)>,
    ) -> EngineResult<()> {
        let now = self.clock.now();
        let before = self.ledger.current_best(&opt.id).await?;
        let mut final_verdict = verdict.to_candidate_verdict();
        let mut final_reasons = reasons.clone();
        let mut moved: Option<(f64, f64)> = None;
        let attempt = {
            let (cid, oid_, aid_) = (cand_id.to_owned(), opt.id.clone(), agent_id.to_owned());
            let (reasons, detail, sha_) = (reasons.clone(), detail.clone(), sha.to_owned());
            let cv = verdict.to_candidate_verdict();
            let confirm_key = k::confirm_best(opt.id.as_str());
            self.ledger
                .atomically(move |mut tx| {
                    Box::pin(async move {
                        let cid = ratchet_core::ids::CandidateId::new(cid);
                        tx.set_verdict(&cid, cv, &reasons, now).await?;
                        let mut moved = None;
                        if cv == CandidateVerdict::Verified
                            && let Some((outcome, confirm)) = &detail
                        {
                            let m = tx
                                .move_ratchet(&oid_, &cid, outcome.candidate_value, now)
                                .await?;
                            moved = Some((m.from_value, m.to_value));
                            if let Some(c) = confirm
                                && !c.is_empty()
                                && let Ok(med) = median(c)
                            {
                                tx.kv_set(&confirm_key, &js_number_to_string(med)).await?;
                            }
                        }
                        tx.append_agent_event(AppendAgentEventInput {
                            agent_id: aid(&aid_),
                            event_type: "verdict".into(),
                            payload: Some(json!({
                                "candidateId": cid.as_str(),
                                "sha": sha_,
                                "verdict": cv.as_str(),
                                "reasons": reasons,
                            })),
                            at: Some(now),
                        })
                        .await?;
                        Ok(moved)
                    })
                })
                .await
        };
        match attempt {
            Ok(m) => moved = m,
            Err(err) if err.code == LedgerErrorCode::RatchetNotImproved => {
                // Lost a race with a better result: the ratchet cannot move backwards, so this is a plain rejection.
                final_verdict = CandidateVerdict::Rejected;
                final_reasons = reasons.clone();
                final_reasons.push(format!(
                    "ratchet already at {}; {}",
                    before.map_or("undefined".to_owned(), js_number_to_string),
                    err.message
                ));
                self.ledger
                    .set_verdict(&cand_id.into(), CandidateVerdict::Rejected, &final_reasons, Some(now))
                    .await?;
            }
            Err(err) => return Err(err.into()),
        }

        self.emit(EngineEvent::Candidate {
            optimization_id: opt.id.clone(),
            candidate_id: cand_id.into(),
            verdict: final_verdict,
        });
        self.emit(EngineEvent::AgentEvent {
            optimization_id: opt.id.clone(),
            agent_id: aid(agent_id),
            event: AgentEvent::new(now, AgentEventType::Verdict)
                .with_text(format!(
                    "{}: {final_verdict}",
                    sha.chars().take(10).collect::<String>()
                ))
                .with_data(json!({
                    "sha": sha,
                    "verdict": final_verdict.as_str(),
                    "reasons": final_reasons,
                })),
        });
        let best = self.ledger.current_best(&opt.id).await?.unwrap_or(0.0);
        if let Some((from, to)) = moved {
            self.emit(EngineEvent::Ratchet {
                optimization_id: opt.id.clone(),
                from,
                to,
            });
        }

        // Tell the submitting agent, and on a ratchet move announce the new shared best to every other runner.
        let agents = self.ledger.list_agents(&opt.id).await?;
        let owner = agents.iter().find(|a| a.id.as_str() == agent_id);
        if let Some(owner) = owner {
            self.send_to(
                owner,
                RunnerMessage::Verdict {
                    sha: sha.to_owned(),
                    verdict: final_verdict,
                    reasons: final_reasons.clone(),
                    current_best: best,
                },
            )
            .await;
        }
        if let (Some((_, to)), Some(owner)) = (moved, owner) {
            for other in &agents {
                if other.id == owner.id {
                    continue;
                }
                self.send_to(
                    other,
                    RunnerMessage::Ratchet {
                        sha: sha.to_owned(),
                        branch: owner.branch.clone(),
                        value: to,
                    },
                )
                .await;
            }
        }
        Ok(())
    }

    async fn send_to(&self, agent: &Agent, msg: RunnerMessage) {
        let Ok(Some(cur)) = self.ledger.get_agent(&agent.id).await else {
            return;
        };
        if cur.runner_instance_id.is_none() || !live_agent_status(cur.status) {
            return;
        }
        let runners = self.runners_for(cur.optimization_id.as_str()).await;
        if let Err(err) = runners.send(&handle(&cur), &msg).await {
            let kind = match &msg {
                RunnerMessage::Verdict { .. } => "verdict",
                RunnerMessage::Ratchet { .. } => "ratchet",
                _ => "message",
            };
            self.report_error(&err, &format!("send {kind} to {}", agent.id));
            let _ = self
                .log_agent(
                    &cur,
                    AgentEvent::new(self.clock.now(), AgentEventType::Error)
                        .with_text(format!("could not deliver {kind}: {err}")),
                )
                .await;
        }
    }

    async fn log_candidate(
        &self,
        agent_id: &str,
        sha: &str,
        text: &str,
        ty: AgentEventType,
    ) -> EngineResult<()> {
        if let Some(agent) = self.ledger.get_agent(&aid(agent_id)).await? {
            self.log_agent(
                &agent,
                AgentEvent::new(self.clock.now(), ty)
                    .with_text(format!("{}: {text}", sha.chars().take(10).collect::<String>())),
            )
            .await?;
        }
        Ok(())
    }

    async fn require_candidate(&self, opt_id: &str, cand_id: &str) -> EngineResult<CandidateWithResults> {
        self.ledger
            .candidates_with_results(&oid(opt_id))
            .await?
            .into_iter()
            .find(|c| c.candidate.id.as_str() == cand_id)
            .ok_or_else(|| EngineError::not_found(format!("candidate {cand_id} not found")))
    }

    // ------------------------------------------------------------------ resume

    /// Re-attach to whatever the ledger says was in flight. Everything the control loop needs is read from the ledger /
    /// kv on each tick, so this only restarts work that was blocking a call when the process died: a baseline in
    /// progress, a half-finished launch, a kill switch that did not finish.
    pub async fn resume(&self) -> EngineResult<ResumeReport> {
        let mut attached = Vec::new();
        for o in self.ledger.list_optimizations(Some(&status_list())).await? {
            let id = o.id.to_string();
            attached.push(id.clone());
            if o.status == OptimizationStatus::Baselining && !lock(&self.starting).contains(&id) {
                // mark it synchronously so a concurrent start() sees it
                lock(&self.starting).insert(id.clone());
                let e = self.clone();
                tokio::spawn(async move {
                    let r = e.run_start(&id).await;
                    lock(&e.starting).remove(&id);
                    if let Err(err) = r {
                        e.report_error(err, &format!("resume start {id}"));
                    }
                });
            } else if o.status == OptimizationStatus::Stopping
                && self.kv.get(&k::stop_mode(&id)).await?.as_deref() == Some("kill")
                && !self.state(&id).killing.load(Ordering::SeqCst)
            {
                let reason = self
                    .kv
                    .get(&k::stop_reason(&id))
                    .await?
                    .unwrap_or_else(|| "stopped".into());
                let e = self.clone();
                tokio::spawn(async move {
                    if let Err(err) = e
                        .execute_kill(&id, &reason, OptimizationStatus::Stopped)
                        .await
                    {
                        e.report_error(err, &format!("resume kill {id}"));
                    }
                });
            } else if o.status == OptimizationStatus::Running
                && let Some(last) = self.last_benched_agent(&id).await?
            {
                *lock(&self.state(&id).last_served_agent) = Some(last);
            }
        }
        Ok(ResumeReport { attached })
    }

    async fn last_benched_agent(&self, id: &str) -> EngineResult<Option<String>> {
        let mut best: Option<(i64, String)> = None;
        for c in self.ledger.candidates_with_results(&oid(id)).await? {
            for r in &c.results {
                if best.as_ref().is_none_or(|(rid, _)| r.id > *rid) {
                    best = Some((r.id, c.candidate.agent_id.to_string()));
                }
            }
        }
        Ok(best.map(|(_, a)| a))
    }
}

/// `status` events may carry `data.state` = done / failed / paused / running; the last one wins.
fn status_from_events(events: &[AgentEvent]) -> Option<AgentStatus> {
    let mut out = None;
    for e in events {
        if e.event_type != AgentEventType::Status {
            continue;
        }
        let state = e
            .data
            .as_ref()
            .and_then(|d| d.get("state"))
            .and_then(|s| s.as_str())
            .map(str::to_owned)
            .or_else(|| finished_state(e.data.as_ref()).map(|s| s.to_string()));
        match state.as_deref() {
            Some("done") => out = Some(AgentStatus::Done),
            Some("failed") => out = Some(AgentStatus::Failed),
            Some("paused") => out = Some(AgentStatus::Paused),
            Some("running") => out = Some(AgentStatus::Running),
            _ => {}
        }
    }
    out
}

/// The runner loop's closing status event carries `{ reason, turns, candidates, ... }` and no `state`: map its stop
/// reason so an agent that ran out of ideas, budget or time counts as done, and one that crashed or could not start as
/// failed. A `stopped` / `aborted` reason is the engine's own doing.
fn finished_state(data: Option<&serde_json::Value>) -> Option<AgentStatus> {
    let data = data?;
    let reason = data.get("reason")?.as_str()?;
    data.get("turns")?.as_f64()?;
    match reason {
        "idle" | "budget" | "deadline" => Some(AgentStatus::Done),
        "failed" | "agent-unavailable" => Some(AgentStatus::Failed),
        _ => None,
    }
}
