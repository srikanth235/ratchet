//! Bench queue (port of `queue.ts`).
//!
//! - One job at a time per optimization (each optimization has its own pinned bench hardware, so different optimizations run
//!   in parallel).
//! - Within an optimization the next job is chosen round-robin over agents: each agent has its own FIFO; agents take turns in
//!   order of first arrival; an agent that still has work goes to the back of the rotation after being served. So an agent
//!   submitting 20 candidates delays another agent's single candidate by at most one job.
//! - Queued jobs are cancelled without ever running; a running job is aborted through its cancellation token (the runner
//!   kills its process groups / terminates its instance).
//!
//! `submit` must be called inside a Tokio runtime (jobs run as tasks).

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};

use indexmap::IndexMap;
use ratchet_core::ports::{BaselineMetrics, BenchJob, BenchOutcome, CancellationToken};
use ratchet_core::{AgentId, OptimizationId, RatchetConfig};
use tokio::sync::oneshot;

use crate::errors::{BenchError, BenchResult};
use crate::pipeline::{BenchProgress, ProgressBenchRunner};

pub struct BenchRequest {
    pub agent_id: AgentId,
    pub job: BenchJob,
    pub config: RatchetConfig,
    pub baseline: Option<BaselineMetrics>,
    /// Cancelling this token cancels the job (queued or running).
    pub cancel: Option<CancellationToken>,
}

/// Who and what an event is about.
#[derive(Debug, Clone, PartialEq)]
pub struct JobRef {
    pub id: String,
    pub agent_id: AgentId,
    pub job: BenchJob,
}

#[derive(Debug, Clone, PartialEq)]
pub enum QueueEvent {
    /// `position` 0 = next to run.
    Queued {
        job: JobRef,
        position: usize,
    },
    Started(JobRef),
    Progress {
        job: JobRef,
        progress: BenchProgress,
    },
    Done {
        job: JobRef,
        outcome: BenchOutcome,
    },
    Failed {
        job: JobRef,
        error: String,
    },
    Cancelled {
        job: JobRef,
        was_running: bool,
    },
}

impl QueueEvent {
    pub fn job(&self) -> &JobRef {
        match self {
            QueueEvent::Queued { job, .. }
            | QueueEvent::Progress { job, .. }
            | QueueEvent::Done { job, .. }
            | QueueEvent::Failed { job, .. }
            | QueueEvent::Cancelled { job, .. } => job,
            QueueEvent::Started(job) => job,
        }
    }
}

/// Short UI label for an event: queued, dependencies, checkout, setup, guardrails 1/2, bench 3/5, confirm 2/3, done, ...
pub fn state_label(e: &QueueEvent) -> String {
    match e {
        QueueEvent::Queued { position, .. } => {
            if *position == 0 {
                "queued (next)".into()
            } else {
                format!("queued (#{})", position + 1)
            }
        }
        QueueEvent::Started(_) => "starting".into(),
        QueueEvent::Progress { progress, .. } => match progress {
            BenchProgress::Deps => "dependencies".into(),
            BenchProgress::Checkout => "checkout".into(),
            BenchProgress::Setup => "setup".into(),
            BenchProgress::Evaluate => "evaluate".into(),
            BenchProgress::Guardrails { index, total, .. } => format!("guardrails {index}/{total}"),
            BenchProgress::Bench { run, of, phase } => {
                let label = match phase {
                    ratchet_core::ports::BenchJobPhase::Confirm => "confirm",
                    ratchet_core::ports::BenchJobPhase::Baseline => "baseline",
                    ratchet_core::ports::BenchJobPhase::Bench => "bench",
                };
                format!("{label} {run}/{of}")
            }
        },
        QueueEvent::Done { .. } => "done".into(),
        QueueEvent::Failed { .. } => "failed".into(),
        QueueEvent::Cancelled { .. } => "cancelled".into(),
    }
}

type Listener = Arc<dyn Fn(&QueueEvent) + Send + Sync>;

struct Entry {
    r: JobRef,
    config: RatchetConfig,
    baseline: Option<BaselineMetrics>,
    controller: CancellationToken,
    flags: Mutex<Flags>,
    tx: Mutex<Option<oneshot::Sender<BenchResult<BenchOutcome>>>>,
    watcher: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

#[derive(Default)]
struct Flags {
    cancelled: bool,
    settled: bool,
}

struct Lane {
    optimization_id: OptimizationId,
    running: Option<Arc<Entry>>,
    per_agent: IndexMap<AgentId, Vec<Arc<Entry>>>,
    rotation: Vec<AgentId>,
}

#[derive(Default)]
struct State {
    lanes: IndexMap<OptimizationId, Lane>,
    seq: u64,
    closed: bool,
}

struct Inner {
    runner: Arc<dyn ProgressBenchRunner>,
    listeners: Mutex<Vec<Listener>>,
    state: Mutex<State>,
    new_id: Option<Box<dyn Fn() -> String + Send + Sync>>,
}

pub struct BenchQueue {
    inner: Arc<Inner>,
}

/// A submitted job: wait for its outcome, or cancel it.
pub struct BenchTicket {
    pub id: String,
    rx: oneshot::Receiver<BenchResult<BenchOutcome>>,
    cancel: Box<dyn Fn() -> bool + Send + Sync>,
}

impl BenchTicket {
    /// Resolves with the outcome; fails with `Cancelled` when cancelled, or the runner's error.
    pub async fn wait(self) -> BenchResult<BenchOutcome> {
        self.rx.await.unwrap_or(Err(BenchError::Cancelled))
    }

    /// Cancel this job. Returns false when it already finished.
    pub fn cancel(&self) -> bool {
        (self.cancel)()
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct LaneSnapshot {
    pub optimization_id: OptimizationId,
    pub running: Option<JobRef>,
    /// In the order jobs will be served.
    pub queued: Vec<(JobRef, usize)>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Inner {
    fn emit(&self, e: &QueueEvent) {
        let listeners: Vec<Listener> = lock(&self.listeners).clone();
        for l in listeners {
            // a broken UI listener must not break the queue
            if catch_unwind(AssertUnwindSafe(|| l(e))).is_err() {
                tracing::warn!(job = %e.job().id, "a bench queue listener panicked; ignored");
            }
        }
    }

    /// The order in which queued entries will run (simulates the rotation without mutating it).
    fn order(lane: &Lane) -> Vec<Arc<Entry>> {
        let mut rotation: Vec<AgentId> = lane.rotation.clone();
        let mut queues: HashMap<&AgentId, Vec<Arc<Entry>>> =
            lane.per_agent.iter().map(|(k, v)| (k, v.clone())).collect();
        let mut out = Vec::new();
        while !rotation.is_empty() {
            let agent = rotation.remove(0);
            let Some(q) = queues.get_mut(&agent) else {
                continue;
            };
            if q.is_empty() {
                continue;
            }
            out.push(q.remove(0));
            if !q.is_empty() {
                rotation.push(agent);
            }
        }
        out
    }

    fn take_next(lane: &mut Lane) -> Option<Arc<Entry>> {
        while !lane.rotation.is_empty() {
            let agent = lane.rotation.remove(0);
            let next = lane.per_agent.get_mut(&agent).and_then(|q| {
                if q.is_empty() {
                    None
                } else {
                    Some(q.remove(0))
                }
            });
            let more = lane.per_agent.get(&agent).is_some_and(|q| !q.is_empty());
            if more {
                lane.rotation.push(agent);
            } else {
                lane.per_agent.shift_remove(&agent);
            }
            if next.is_some() {
                return next;
            }
        }
        None
    }

    fn is_queued(state: &State, entry: &Entry) -> bool {
        state
            .lanes
            .get(&entry.r.job.optimization_id)
            .and_then(|l| l.per_agent.get(&entry.r.agent_id))
            .is_some_and(|q| q.iter().any(|e| std::ptr::eq(&**e, entry)))
    }
}

fn pump(inner: &Arc<Inner>, opt: &OptimizationId) {
    let entry = {
        let mut st = lock(&inner.state);
        let Some(lane) = st.lanes.get_mut(opt) else {
            return;
        };
        if lane.running.is_some() {
            return;
        }
        let Some(entry) = Inner::take_next(lane) else {
            return;
        };
        lane.running = Some(entry.clone());
        entry
    };
    let inner = inner.clone();
    tokio::spawn(async move {
        execute(&inner, &entry).await;
        {
            let mut st = lock(&inner.state);
            if let Some(lane) = st.lanes.get_mut(&entry.r.job.optimization_id) {
                lane.running = None;
            }
        }
        pump(&inner, &entry.r.job.optimization_id);
    });
}

async fn execute(inner: &Arc<Inner>, entry: &Arc<Entry>) {
    inner.emit(&QueueEvent::Started(entry.r.clone()));
    // our runners take an optional progress callback; a plain BenchRunner ignores it (see `NoProgress`)
    let progress = {
        let inner = inner.clone();
        let r = entry.r.clone();
        move |p: BenchProgress| {
            inner.emit(&QueueEvent::Progress {
                job: r.clone(),
                progress: p,
            });
        }
    };
    let result = inner
        .runner
        .run_with_progress(
            &entry.r.job,
            &entry.config,
            entry.baseline.as_ref(),
            Some(&entry.controller),
            Some(&progress),
        )
        .await;
    let cancelled = lock(&entry.flags).cancelled;
    match result {
        Ok(outcome) => {
            if cancelled {
                settle(inner, entry, Settle::Cancelled { was_running: true });
            } else {
                inner.emit(&QueueEvent::Done {
                    job: entry.r.clone(),
                    outcome: outcome.clone(),
                });
                settle(inner, entry, Settle::Ok(outcome));
            }
        }
        Err(err) => {
            if cancelled {
                settle(inner, entry, Settle::Cancelled { was_running: true });
            } else {
                inner.emit(&QueueEvent::Failed {
                    job: entry.r.clone(),
                    error: err.to_string(),
                });
                settle(inner, entry, Settle::Err(err));
            }
        }
    }
}

enum Settle {
    Ok(BenchOutcome),
    Err(BenchError),
    Cancelled { was_running: bool },
}

fn settle(inner: &Arc<Inner>, entry: &Arc<Entry>, how: Settle) {
    {
        let mut f = lock(&entry.flags);
        if f.settled {
            return;
        }
        f.settled = true;
    }
    if let Some(w) = lock(&entry.watcher).take() {
        w.abort();
    }
    let value = match how {
        Settle::Ok(o) => Ok(o),
        Settle::Err(e) => Err(e),
        Settle::Cancelled { was_running } => {
            inner.emit(&QueueEvent::Cancelled {
                job: entry.r.clone(),
                was_running,
            });
            Err(BenchError::Cancelled)
        }
    };
    if let Some(tx) = lock(&entry.tx).take() {
        let _ = tx.send(value);
    }
}

/// Cancel one entry. Returns false when it already finished.
fn cancel_entry(inner: &Arc<Inner>, entry: &Arc<Entry>) -> bool {
    enum Action {
        Nothing,
        AbortRunning,
        SettleQueued,
    }
    let action = {
        let mut st = lock(&inner.state);
        let queued = Inner::is_queued(&st, entry);
        let mut f = lock(&entry.flags);
        if f.settled || (f.cancelled && !queued) {
            Action::Nothing
        } else {
            f.cancelled = true;
            drop(f);
            let opt = entry.r.job.optimization_id.clone();
            let lane = st.lanes.entry(opt.clone()).or_insert_with(|| Lane {
                optimization_id: opt,
                running: None,
                per_agent: IndexMap::new(),
                rotation: Vec::new(),
            });
            if lane.running.as_ref().is_some_and(|r| Arc::ptr_eq(r, entry)) {
                Action::AbortRunning
            } else {
                if let Some(q) = lane.per_agent.get_mut(&entry.r.agent_id) {
                    q.retain(|e| !Arc::ptr_eq(e, entry));
                    if q.is_empty() {
                        lane.per_agent.shift_remove(&entry.r.agent_id);
                        let agent = entry.r.agent_id.clone();
                        lane.rotation.retain(|a| *a != agent);
                    }
                }
                Action::SettleQueued
            }
        }
    };
    match action {
        Action::Nothing => false,
        Action::AbortRunning => {
            entry.controller.cancel(); // execute() settles once the runner stops
            true
        }
        Action::SettleQueued => {
            settle(inner, entry, Settle::Cancelled { was_running: false });
            true
        }
    }
}

impl BenchQueue {
    pub fn new(runner: Arc<dyn ProgressBenchRunner>) -> Self {
        Self::with_options(runner, None, None)
    }

    /// `on_event` receives every event; `new_id` generates job ids (tests).
    pub fn with_options(
        runner: Arc<dyn ProgressBenchRunner>,
        on_event: Option<Listener>,
        new_id: Option<Box<dyn Fn() -> String + Send + Sync>>,
    ) -> Self {
        BenchQueue {
            inner: Arc::new(Inner {
                runner,
                listeners: Mutex::new(on_event.into_iter().collect()),
                state: Mutex::new(State::default()),
                new_id,
            }),
        }
    }

    /// Subscribe to events; returns a handle that is not needed to keep the subscription (use `unsubscribe` semantics by
    /// dropping the queue).
    pub fn subscribe(&self, listener: Listener) {
        lock(&self.inner.listeners).push(listener);
    }

    pub fn submit(&self, req: BenchRequest) -> Result<BenchTicket, BenchError> {
        let inner = &self.inner;
        let id = {
            let mut st = lock(&inner.state);
            if st.closed {
                return Err(BenchError::infra("BenchQueue is closed"));
            }
            st.seq += 1;
            match &inner.new_id {
                Some(f) => f(),
                None => format!("bench-{}", st.seq),
            }
        };
        let (tx, rx) = oneshot::channel();
        let pre_cancelled = req
            .cancel
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled);
        let entry = Arc::new(Entry {
            r: JobRef {
                id: id.clone(),
                agent_id: req.agent_id.clone(),
                job: req.job.clone(),
            },
            config: req.config,
            baseline: req.baseline,
            controller: CancellationToken::new(),
            flags: Mutex::new(Flags {
                cancelled: pre_cancelled,
                settled: false,
            }),
            tx: Mutex::new(Some(tx)),
            watcher: Mutex::new(None),
        });
        if let Some(external) = req.cancel
            && !pre_cancelled
        {
            let (inner2, entry2) = (inner.clone(), entry.clone());
            *lock(&entry.watcher) = Some(tokio::spawn(async move {
                external.cancelled().await;
                cancel_entry(&inner2, &entry2);
            }));
        }

        let position = {
            let mut st = lock(&inner.state);
            let opt = entry.r.job.optimization_id.clone();
            let lane = st.lanes.entry(opt.clone()).or_insert_with(|| Lane {
                optimization_id: opt,
                running: None,
                per_agent: IndexMap::new(),
                rotation: Vec::new(),
            });
            lane.per_agent
                .entry(entry.r.agent_id.clone())
                .or_default()
                .push(entry.clone());
            if !lane.rotation.contains(&entry.r.agent_id) {
                lane.rotation.push(entry.r.agent_id.clone());
            }
            Inner::order(lane)
                .iter()
                .position(|e| Arc::ptr_eq(e, &entry))
                .unwrap_or(0)
        };
        inner.emit(&QueueEvent::Queued {
            job: entry.r.clone(),
            position,
        });
        if lock(&entry.flags).cancelled {
            cancel_entry(inner, &entry);
        } else {
            pump(inner, &entry.r.job.optimization_id);
        }
        let (inner3, entry3) = (inner.clone(), entry.clone());
        Ok(BenchTicket {
            id,
            rx,
            cancel: Box::new(move || cancel_entry(&inner3, &entry3)),
        })
    }

    fn find(&self, pred: impl Fn(&Arc<Entry>) -> bool) -> Vec<Arc<Entry>> {
        let st = lock(&self.inner.state);
        let mut out = Vec::new();
        for lane in st.lanes.values() {
            if let Some(r) = &lane.running
                && pred(r)
            {
                out.push(r.clone());
            }
            for q in lane.per_agent.values() {
                out.extend(q.iter().filter(|e| pred(e)).cloned());
            }
        }
        out
    }

    /// Cancel one job by id. Returns false when it is unknown or already finished.
    pub fn cancel(&self, id: &str) -> bool {
        match self.find(|e| e.r.id == id).first() {
            Some(e) => cancel_entry(&self.inner, e),
            None => false,
        }
    }

    fn cancel_where(&self, pred: impl Fn(&Arc<Entry>) -> bool) -> usize {
        let targets = self.find(pred);
        let n = targets.len();
        for t in targets {
            cancel_entry(&self.inner, &t);
        }
        n
    }

    /// Cancel everything queued or running for an agent (optionally within one optimization).
    pub fn cancel_agent(
        &self,
        agent_id: &AgentId,
        optimization_id: Option<&OptimizationId>,
    ) -> usize {
        self.cancel_where(|e| {
            e.r.agent_id == *agent_id
                && optimization_id.is_none_or(|o| e.r.job.optimization_id == *o)
        })
    }

    pub fn cancel_optimization(&self, optimization_id: &OptimizationId) -> usize {
        self.cancel_where(|e| e.r.job.optimization_id == *optimization_id)
    }

    /// Cancel everything and refuse new submissions.
    pub fn close(&self) {
        lock(&self.inner.state).closed = true;
        self.cancel_where(|_| true);
    }

    /// What is running and what is waiting, in the order jobs will be served.
    pub fn snapshot(&self) -> Vec<LaneSnapshot> {
        let st = lock(&self.inner.state);
        st.lanes
            .values()
            .map(|lane| LaneSnapshot {
                optimization_id: lane.optimization_id.clone(),
                running: lane.running.as_ref().map(|e| e.r.clone()),
                queued: Inner::order(lane)
                    .into_iter()
                    .enumerate()
                    .map(|(i, e)| (e.r.clone(), i))
                    .collect(),
            })
            .collect()
    }
}
