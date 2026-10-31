//! Port of `packages/bench/src/queue.test.ts`.

use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ratchet_bench::pipeline::ProgressFn;
use ratchet_bench::testkit::{ConfigOptions, make_config};
use ratchet_bench::{
    BenchError, BenchProgress, BenchQueue, BenchRequest, BenchResult, BenchTicket,
    ProgressBenchRunner, QueueEvent, state_label,
};
use ratchet_core::ports::{
    BaselineMetrics, BenchJob, BenchJobPhase, BenchOutcome, BenchRunner, CancellationToken,
    PortResult,
};
use ratchet_core::{AgentId, OptimizationId, RatchetConfig};
use tokio::sync::{mpsc, oneshot};

fn outcome(v: f64) -> BenchOutcome {
    BenchOutcome {
        runs: vec![],
        samples: vec![v],
        guardrails: vec![],
        started_at: 0,
        finished_at: 1,
        logs_key: None,
    }
}

struct Call {
    job: BenchJob,
    cancel: Option<CancellationToken>,
    progress: mpsc::UnboundedSender<BenchProgress>,
    done: Mutex<Option<oneshot::Sender<BenchResult<BenchOutcome>>>>,
}

impl Call {
    fn resolve(&self, o: Option<BenchOutcome>) {
        if let Some(tx) = self.done.lock().expect("lock").take() {
            let _ = tx.send(Ok(o.unwrap_or_else(|| outcome(1.0))));
        }
    }

    fn reject(&self, e: BenchError) {
        if let Some(tx) = self.done.lock().expect("lock").take() {
            let _ = tx.send(Err(e));
        }
    }

    fn aborted(&self) -> bool {
        self.cancel
            .as_ref()
            .is_some_and(CancellationToken::is_cancelled)
    }
}

/// A runner whose jobs finish when the test says so. Aborting rejects the job like a real runner.
#[derive(Default)]
struct ControlledRunner {
    calls: Mutex<Vec<Arc<Call>>>,
}

impl ControlledRunner {
    fn call(&self, i: usize) -> Arc<Call> {
        self.calls.lock().expect("lock")[i].clone()
    }

    fn started(&self) -> Vec<String> {
        self.calls
            .lock()
            .expect("lock")
            .iter()
            .map(|c| {
                c.job
                    .candidate_id
                    .as_ref()
                    .map_or_else(|| c.job.sha.clone(), |id| id.to_string())
            })
            .collect()
    }
}

#[async_trait]
impl BenchRunner for ControlledRunner {
    async fn run(
        &self,
        job: &BenchJob,
        config: &RatchetConfig,
        baseline: Option<&BaselineMetrics>,
        cancel: Option<&CancellationToken>,
    ) -> PortResult<BenchOutcome> {
        Ok(self
            .run_with_progress(job, config, baseline, cancel, None)
            .await?)
    }
}

#[async_trait]
impl ProgressBenchRunner for ControlledRunner {
    async fn run_with_progress(
        &self,
        job: &BenchJob,
        _config: &RatchetConfig,
        _baseline: Option<&BaselineMetrics>,
        cancel: Option<&CancellationToken>,
        on_progress: Option<ProgressFn<'_>>,
    ) -> BenchResult<BenchOutcome> {
        let (done_tx, mut done_rx) = oneshot::channel();
        let (progress_tx, mut progress_rx) = mpsc::unbounded_channel();
        self.calls.lock().expect("lock").push(Arc::new(Call {
            job: job.clone(),
            cancel: cancel.cloned(),
            progress: progress_tx,
            done: Mutex::new(Some(done_tx)),
        }));
        let cancelled = async {
            match cancel {
                Some(c) => c.cancelled().await,
                None => std::future::pending::<()>().await,
            }
        };
        tokio::pin!(cancelled);
        loop {
            tokio::select! {
                Some(p) = progress_rx.recv() => {
                    if let Some(f) = on_progress { f(p); }
                }
                r = &mut done_rx => return r.unwrap_or(Err(BenchError::Aborted)),
                () = &mut cancelled => return Err(BenchError::Aborted),
            }
        }
    }
}

fn job(id: &str, optimization: &str) -> BenchJob {
    BenchJob {
        optimization_id: OptimizationId::new(optimization),
        candidate_id: Some(ratchet_core::CandidateId::new(id)),
        sha: id.into(),
        phase: BenchJobPhase::Bench,
        repeats: 2,
        trace: None,
    }
}

async fn tick() {
    tokio::time::sleep(Duration::from_millis(15)).await;
}

struct Setup {
    runner: Arc<ControlledRunner>,
    events: Arc<Mutex<Vec<QueueEvent>>>,
    queue: BenchQueue,
    config: RatchetConfig,
}

fn setup() -> Setup {
    let runner = Arc::new(ControlledRunner::default());
    let events: Arc<Mutex<Vec<QueueEvent>>> = Arc::default();
    let sink = events.clone();
    let queue = BenchQueue::with_options(
        runner.clone(),
        Some(Arc::new(move |e: &QueueEvent| {
            sink.lock().expect("lock").push(e.clone())
        })),
        None,
    );
    Setup {
        runner,
        events,
        queue,
        config: make_config(&ConfigOptions::default()),
    }
}

impl Setup {
    fn submit(&self, agent: &str, id: &str, opt: &str) -> BenchTicket {
        self.queue
            .submit(BenchRequest {
                agent_id: AgentId::new(agent),
                job: job(id, opt),
                config: self.config.clone(),
                baseline: None,
                cancel: None,
            })
            .expect("submit")
    }

    fn a(&self, agent: &str, id: &str) -> BenchTicket {
        self.submit(agent, id, "opt1")
    }

    fn events(&self) -> Vec<QueueEvent> {
        self.events.lock().expect("lock").clone()
    }
}

#[tokio::test]
async fn runs_one_job_at_a_time_per_optimization_fifo() {
    let s = setup();
    let t1 = s.a("A", "a1");
    let t2 = s.a("A", "a2");
    s.a("A", "a3");
    tick().await;
    assert_eq!(s.runner.started(), ["a1"]);
    s.runner.call(0).resolve(Some(outcome(11.0)));
    assert_eq!(t1.wait().await.expect("outcome").samples, [11.0]);
    tick().await;
    assert_eq!(s.runner.started(), ["a1", "a2"]);
    s.runner.call(1).resolve(None);
    t2.wait().await.expect("outcome");
    tick().await;
    assert_eq!(s.runner.started(), ["a1", "a2", "a3"]);
}

#[tokio::test]
async fn runs_different_optimizations_in_parallel() {
    let s = setup();
    s.submit("A", "x1", "optX");
    s.submit("B", "y1", "optY");
    s.submit("A", "x2", "optX");
    tick().await;
    assert_eq!(s.runner.started(), ["x1", "y1"]);
}

#[tokio::test]
async fn is_fair_across_agents_round_robin() {
    let s = setup();
    s.a("A", "a1");
    for id in ["a2", "a3", "a4", "a5"] {
        s.a("A", id);
    }
    s.a("B", "b1");
    s.a("C", "c1");
    s.a("C", "c2");
    let mut order = Vec::new();
    // drain: finish whatever runs, one at a time
    for i in 0..8 {
        tick().await;
        let call = s.runner.call(i);
        order.push(call.job.candidate_id.as_ref().expect("id").to_string());
        call.resolve(None);
    }
    // A's backlog does not delay B and C by more than one job each
    assert_eq!(order, ["a1", "a2", "b1", "c1", "a3", "c2", "a4", "a5"]);
}

#[tokio::test]
async fn serves_a_late_agent_after_at_most_one_more_job_of_the_busy_agent() {
    let s = setup();
    for id in ["a1", "a2", "a3", "a4"] {
        s.a("A", id);
    }
    tick().await;
    s.a("B", "b1");
    s.runner.call(0).resolve(None);
    tick().await;
    assert_eq!(s.runner.started(), ["a1", "a2"]);
    s.runner.call(1).resolve(None);
    tick().await;
    assert_eq!(s.runner.started(), ["a1", "a2", "b1"]);
}

#[tokio::test]
async fn cancels_a_queued_job_without_running_it() {
    let s = setup();
    s.a("A", "a1");
    let t2 = s.a("A", "a2");
    let t3 = s.a("B", "b1");
    tick().await;
    let t2_id = t2.id.clone();
    t2.cancel();
    assert_eq!(
        t2.wait().await.expect_err("cancelled"),
        BenchError::Cancelled
    );
    s.runner.call(0).resolve(None);
    tick().await;
    assert_eq!(s.runner.started(), ["a1", "b1"]);
    let cancelled: Vec<_> = s
        .events()
        .into_iter()
        .filter_map(|e| match e {
            QueueEvent::Cancelled { job, was_running } => Some((job.id, was_running)),
            _ => None,
        })
        .collect();
    assert_eq!(cancelled, [(t2_id, false)]);
    s.runner.call(1).resolve(None);
    t3.wait().await.expect("outcome");
}

#[tokio::test]
async fn aborts_a_running_job_and_moves_on_to_the_next() {
    let s = setup();
    let t1 = s.a("A", "a1");
    s.a("B", "b1");
    tick().await;
    assert!(!s.runner.call(0).aborted());
    t1.cancel();
    assert!(s.runner.call(0).aborted());
    assert_eq!(
        t1.wait().await.expect_err("cancelled"),
        BenchError::Cancelled
    );
    tick().await;
    assert_eq!(s.runner.started(), ["a1", "b1"]);
    let events = s.events();
    assert!(events.iter().any(|e| matches!(
        e,
        QueueEvent::Cancelled {
            was_running: true,
            ..
        }
    )));
    assert!(
        !events
            .iter()
            .any(|e| matches!(e, QueueEvent::Failed { .. }))
    );
}

#[tokio::test]
async fn cancel_agent_cancel_optimization_and_close() {
    let s = setup();
    let a1 = s.a("A", "a1");
    let a2 = s.a("A", "a2");
    let b1 = s.a("B", "b1");
    let x1 = s.submit("A", "x1", "optX");
    tick().await;
    assert_eq!(
        s.queue
            .cancel_agent(&AgentId::new("A"), Some(&OptimizationId::new("opt1"))),
        2
    ); // running a1 + queued a2
    assert_eq!(
        a1.wait().await.expect_err("cancelled"),
        BenchError::Cancelled
    );
    assert_eq!(
        a2.wait().await.expect_err("cancelled"),
        BenchError::Cancelled
    );
    tick().await;
    assert_eq!(s.runner.started(), ["a1", "x1", "b1"]);
    assert_eq!(s.queue.cancel_optimization(&OptimizationId::new("optX")), 1);
    assert_eq!(
        x1.wait().await.expect_err("cancelled"),
        BenchError::Cancelled
    );
    s.queue.close();
    assert_eq!(
        b1.wait().await.expect_err("cancelled"),
        BenchError::Cancelled
    );
    let late = s
        .queue
        .submit(BenchRequest {
            agent_id: AgentId::new("A"),
            job: job("late", "opt1"),
            config: s.config.clone(),
            baseline: None,
            cancel: None,
        })
        .map(|_| ())
        .expect_err("closed");
    assert!(late.to_string().contains("closed"));
    assert!(!s.queue.cancel("nope"));
}

#[tokio::test]
async fn honours_an_external_cancellation_token_even_one_already_cancelled() {
    let s = setup();
    let external = CancellationToken::new();
    s.a("A", "a1");
    let t2 = s
        .queue
        .submit(BenchRequest {
            agent_id: AgentId::new("A"),
            job: job("a2", "opt1"),
            config: s.config.clone(),
            baseline: None,
            cancel: Some(external.clone()),
        })
        .expect("submit");
    external.cancel();
    assert_eq!(
        t2.wait().await.expect_err("cancelled"),
        BenchError::Cancelled
    );
    let pre = CancellationToken::new();
    pre.cancel();
    let t3 = s
        .queue
        .submit(BenchRequest {
            agent_id: AgentId::new("A"),
            job: job("a3", "opt1"),
            config: s.config.clone(),
            baseline: None,
            cancel: Some(pre),
        })
        .expect("submit");
    assert_eq!(
        t3.wait().await.expect_err("cancelled"),
        BenchError::Cancelled
    );
    s.runner.call(0).resolve(None);
    tick().await;
    assert_eq!(s.runner.started(), ["a1"]);
}

#[tokio::test]
async fn emits_queued_started_progress_and_done_events_with_ui_labels() {
    let s = setup();
    s.a("A", "a1");
    tick().await;
    let p = s.runner.call(0).progress.clone();
    for step in [
        BenchProgress::Deps,
        BenchProgress::Checkout,
        BenchProgress::Guardrails {
            index: 1,
            total: 2,
            name: "pnpm test".into(),
        },
        BenchProgress::Bench {
            run: 2,
            of: 5,
            phase: BenchJobPhase::Bench,
        },
        BenchProgress::Bench {
            run: 1,
            of: 3,
            phase: BenchJobPhase::Confirm,
        },
    ] {
        p.send(step).expect("send");
    }
    tick().await;
    s.runner.call(0).resolve(None);
    tick().await;
    let labels: Vec<String> = s.events().iter().map(state_label).collect();
    assert_eq!(
        labels,
        [
            "queued (next)",
            "starting",
            "dependencies",
            "checkout",
            "guardrails 1/2",
            "bench 2/5",
            "confirm 1/3",
            "done"
        ]
    );
}

#[tokio::test]
async fn reports_a_failing_runner_as_a_failed_event_and_keeps_going() {
    let s = setup();
    let t1 = s.a("A", "a1");
    let t2 = s.a("A", "a2");
    tick().await;
    s.runner
        .call(0)
        .reject(BenchError::infra("instance exploded"));
    assert_eq!(
        t1.wait().await.expect_err("failed").to_string(),
        "instance exploded"
    );
    tick().await;
    let failed = s.events().into_iter().find_map(|e| match e {
        QueueEvent::Failed { error, .. } => Some(error),
        _ => None,
    });
    assert_eq!(failed.as_deref(), Some("instance exploded"));
    assert_eq!(s.runner.started(), ["a1", "a2"]);
    s.runner.call(1).resolve(None);
    t2.wait().await.expect("outcome");
}

#[tokio::test]
async fn snapshot_lists_the_running_job_and_the_waiting_ones_in_service_order() {
    let s = setup();
    s.a("A", "a1");
    s.a("A", "a2");
    s.a("A", "a3");
    s.a("B", "b1");
    tick().await;
    let snap = s.queue.snapshot();
    let lane = &snap[0];
    assert_eq!(
        lane.running
            .as_ref()
            .and_then(|r| r.job.candidate_id.as_ref())
            .map(ToString::to_string),
        Some("a1".into())
    );
    let queued: Vec<(String, usize)> = lane
        .queued
        .iter()
        .map(|(r, pos)| (r.job.candidate_id.as_ref().expect("id").to_string(), *pos))
        .collect();
    assert_eq!(
        queued,
        [
            ("a2".to_owned(), 0),
            ("b1".to_owned(), 1),
            ("a3".to_owned(), 2)
        ]
    );
}

#[tokio::test]
async fn survives_a_throwing_listener() {
    let runner = Arc::new(ControlledRunner::default());
    let queue = BenchQueue::with_options(
        runner.clone(),
        Some(Arc::new(|_: &QueueEvent| panic!("ui bug"))),
        None,
    );
    let config = make_config(&ConfigOptions::default());
    let t = queue
        .submit(BenchRequest {
            agent_id: AgentId::new("A"),
            job: job("a1", "opt1"),
            config,
            baseline: None,
            cancel: None,
        })
        .expect("submit");
    tick().await;
    runner.call(0).resolve(None);
    assert!(t.wait().await.is_ok());
}
