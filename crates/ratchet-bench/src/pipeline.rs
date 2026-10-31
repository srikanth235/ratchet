//! The bench pipeline, shared by `LocalBenchRunner` and `RemoteBenchRunner` so both run the same steps (port of
//! `pipeline.ts`):
//!
//! ```text
//! checkout (+ protected overlay; a changed lockfile blocks here) -> setup -> command guardrails (fail fast) -> bench x N
//!   -> threshold guardrails over the per-metric medians of the N runs
//! ```
//!
//! Only the [`PipelineExecutor`] differs (local child processes vs. SSM scripts on a bench instance).

use std::collections::BTreeMap;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use ratchet_core::config::{CommandGuardrail, ThresholdGuardrail};
use ratchet_core::ports::{
    ArtifactStore, BaselineMetrics, BenchJob, BenchJobPhase, BenchOutcome, BenchRun, BenchRunner, CancellationToken,
    Clock, PortResult,
};
use ratchet_core::config::evaluate_threshold;
use ratchet_core::{Guardrail, GuardrailResult, RatchetConfig, extract_last_json_object, lookup_path};
use serde_json::Value;

use crate::errors::{BenchError, BenchResult};
use crate::flatten::{flatten_numeric, median_metrics, resolve_metric};
use crate::log::BenchLog;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StepTimeouts {
    pub checkout_ms: u64,
    /// The dependency-snapshot fetch (remote runner only; it runs once per lockfile, not per job).
    pub deps_ms: u64,
    /// Building the repo's container image (remote runner only; once per Dockerfile / devcontainer, not per job).
    pub image_ms: u64,
    pub setup_ms: u64,
    pub guardrail_ms: u64,
    /// Per single `bench.run` invocation.
    pub bench_ms: u64,
}

pub const DEFAULT_TIMEOUTS: StepTimeouts = StepTimeouts {
    checkout_ms: 10 * 60_000,
    deps_ms: 20 * 60_000,
    image_ms: 45 * 60_000,
    setup_ms: 30 * 60_000,
    guardrail_ms: 15 * 60_000,
    bench_ms: 30 * 60_000,
};

impl Default for StepTimeouts {
    fn default() -> Self {
        DEFAULT_TIMEOUTS
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BenchProgress {
    Deps,
    Checkout,
    Setup,
    Guardrails { index: usize, total: usize, name: String },
    Bench { run: u64, of: u64, phase: BenchJobPhase },
    Evaluate,
}

/// Callback for step progress (the queue turns it into UI states).
pub type ProgressFn<'a> = &'a (dyn Fn(BenchProgress) + Send + Sync);

/// A `BenchRunner` that also reports step progress (the queue turns it into UI states).
#[async_trait]
pub trait ProgressBenchRunner: BenchRunner {
    async fn run_with_progress(
        &self,
        job: &BenchJob,
        config: &RatchetConfig,
        baseline: Option<&BaselineMetrics>,
        cancel: Option<&CancellationToken>,
        on_progress: Option<ProgressFn<'_>>,
    ) -> BenchResult<BenchOutcome>;
}

/// Adapts any `BenchRunner` to the queue's `ProgressBenchRunner` (it reports no progress).
pub struct NoProgress(pub Arc<dyn BenchRunner>);

#[async_trait]
impl BenchRunner for NoProgress {
    async fn run(
        &self,
        job: &BenchJob,
        config: &RatchetConfig,
        baseline: Option<&BaselineMetrics>,
        cancel: Option<&CancellationToken>,
    ) -> PortResult<BenchOutcome> {
        self.0.run(job, config, baseline, cancel).await
    }
}

#[async_trait]
impl ProgressBenchRunner for NoProgress {
    async fn run_with_progress(
        &self,
        job: &BenchJob,
        config: &RatchetConfig,
        baseline: Option<&BaselineMetrics>,
        cancel: Option<&CancellationToken>,
        _on_progress: Option<ProgressFn<'_>>,
    ) -> BenchResult<BenchOutcome> {
        self.0
            .run(job, config, baseline, cancel)
            .await
            .map_err(BenchError::from)
    }
}

pub use crate::remote_scripts::StepKind;

#[derive(Debug, Clone)]
pub struct StepSpec {
    pub kind: StepKind,
    pub name: String,
    pub command: String,
    pub timeout_ms: u64,
    /// Extra environment (RATCHET_* variables), in order.
    pub env: indexmap::IndexMap<String, String>,
    /// Confirm bench steps only: the hidden trace reference. The executor makes it available to the evaluator for that step
    /// alone and exports it as `RATCHET_TRACE`; the remote executor fetches it after setup and guardrails, readable by the
    /// harness user only, and wipes it when the step ends.
    pub trace: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct StepOutcome {
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    /// (Tail of the) stdout; the bench JSON is parsed from here.
    pub stdout: String,
    pub duration_ms: f64,
    /// Set when trusted files were modified by an earlier step (restored; the step was NOT run).
    pub tampered: Option<Vec<String>>,
    /// Set when the step could not be started.
    pub error: Option<String>,
}

/// An executor refuses to go on: the pipeline records `guardrail` (failed) and the candidate is blocked without running
/// anything.
#[derive(Debug, Clone)]
pub struct CheckoutVeto {
    pub guardrail: GuardrailResult,
}

#[async_trait]
pub trait PipelineExecutor: Send {
    /// Materialize the candidate and overlay the protected files from bench.ref. `trace` is only announced here: the holdout
    /// is not fetched before the bench steps. May return a veto (a candidate whose lockfile differs from bench.ref's: the
    /// bench has no network, so its dependencies cannot be installed).
    async fn checkout(&mut self, trace: Option<&str>) -> BenchResult<Option<CheckoutVeto>>;
    /// Extra explanation for a failed setup (e.g. that the bench installs from a snapshot with no network).
    fn setup_failure_hint(&self) -> Option<String> {
        None
    }
    /// Run one step. Fails with `Aborted` when aborted. Must verify/restore protected files first.
    async fn run_step(&mut self, spec: &StepSpec) -> BenchResult<StepOutcome>;
    async fn dispose(&mut self);
}

pub struct PipelineContext<'a> {
    pub job: &'a BenchJob,
    pub config: &'a RatchetConfig,
    pub baseline: Option<&'a BaselineMetrics>,
    pub exec: &'a mut dyn PipelineExecutor,
    pub log: &'a BenchLog,
    pub setup: Option<&'a str>,
    pub timeouts: StepTimeouts,
    /// Run command guardrails in the confirm phase too (default false: they passed in the bench phase).
    pub command_guardrails_in_confirm: bool,
    pub cancel: Option<&'a CancellationToken>,
    pub on_progress: Option<ProgressFn<'a>>,
}

#[derive(Debug, Clone, Default)]
pub struct PipelineResult {
    pub runs: Vec<BenchRun>,
    pub guardrails: Vec<GuardrailResult>,
}

pub fn throw_if_aborted(cancel: Option<&CancellationToken>) -> BenchResult<()> {
    if cancel.is_some_and(CancellationToken::is_cancelled) {
        Err(BenchError::Aborted)
    } else {
        Ok(())
    }
}

pub(crate) fn phase_str(p: BenchJobPhase) -> &'static str {
    match p {
        BenchJobPhase::Baseline => "baseline",
        BenchJobPhase::Bench => "bench",
        BenchJobPhase::Confirm => "confirm",
    }
}

/// The hidden trace of a config (`bench.confirm_trace`, i.e. `evaluator.holdout`).
pub fn config_trace(config: &RatchetConfig) -> Option<String> {
    config
        .evaluator
        .confirm_trace
        .clone()
        .or_else(|| config.evaluator.holdout.clone())
}

#[derive(Debug, Clone, PartialEq)]
pub struct ParsedBench {
    pub value: f64,
    pub metrics: BTreeMap<String, f64>,
}

/// Parse one `bench.run` stdout: the last JSON object; objective value by dotted path; all numeric leaves as metrics.
pub fn parse_bench_output(stdout: &str, metric_path: &str) -> Result<ParsedBench, String> {
    let Some(obj) = extract_last_json_object(stdout) else {
        return Err("no JSON object found on stdout".into());
    };
    let root = Value::Object(obj);
    let Some(v) = lookup_path(&root, metric_path) else {
        return Err(format!(
            "objective metric \"{metric_path}\" not found in JSON output"
        ));
    };
    let value = match v.as_f64() {
        Some(n) if n.is_finite() && v.is_number() => n,
        _ => {
            return Err(format!(
                "objective metric \"{metric_path}\" is not a finite number (got {})",
                serde_json::to_string(v).unwrap_or_default()
            ));
        }
    };
    let mut metrics = flatten_numeric(&root);
    metrics.insert(metric_path.to_owned(), value);
    Ok(ParsedBench { value, metrics })
}

/// Threshold guardrails over the per-metric medians of `runs`, relative bounds against `baseline.metrics`.
pub fn evaluate_thresholds(
    guardrails: &[ThresholdGuardrail],
    runs: &[BenchRun],
    baseline: Option<&BaselineMetrics>,
    phase: BenchJobPhase,
) -> Vec<GuardrailResult> {
    let medians = median_metrics(runs.iter().map(|r| &r.metrics));
    guardrails
        .iter()
        .map(|g| {
            let name = Guardrail::Threshold(g.clone()).name();
            if g.relative && phase == BenchJobPhase::Baseline {
                return GuardrailResult {
                    name,
                    passed: true,
                    detail: Some("skipped: relative bounds are measured against this baseline run".into()),
                };
            }
            let Some((key, value)) = resolve_metric(&medians, &g.metric) else {
                return GuardrailResult {
                    name,
                    passed: false,
                    detail: Some(format!(
                        "metric \"{}\" was not reported by the benchmark (reported: {})",
                        g.metric,
                        if medians.is_empty() {
                            "nothing".to_owned()
                        } else {
                            medians.keys().cloned().collect::<Vec<_>>().join(", ")
                        }
                    )),
                };
            };
            let base = baseline.and_then(|b| resolve_metric(&b.metrics, &g.metric).map(|(_, v)| v));
            let mut res = evaluate_threshold(g, value, base);
            if key != g.metric {
                res.detail = res.detail.map(|d| d.replacen(&g.metric, &key, 1));
            }
            res
        })
        .collect()
}

pub fn command_guardrail_result(g: &CommandGuardrail, out: &StepOutcome, timeout_ms: u64) -> GuardrailResult {
    let name = g.run.clone();
    if out.timed_out {
        return GuardrailResult {
            name,
            passed: false,
            detail: Some(format!("timed out after {timeout_ms}ms")),
        };
    }
    if let Some(e) = &out.error {
        return GuardrailResult {
            name,
            passed: false,
            detail: Some(e.clone()),
        };
    }
    match out.exit_code {
        None => GuardrailResult {
            name,
            passed: false,
            detail: Some("terminated by a signal".into()),
        },
        Some(0) => GuardrailResult {
            name,
            passed: true,
            detail: None,
        },
        Some(c) => GuardrailResult {
            name,
            passed: false,
            detail: Some(format!("exited with status {c}")),
        },
    }
}

fn tampered_guardrail(paths: &[String]) -> GuardrailResult {
    let mut shown = paths.iter().take(5).cloned().collect::<Vec<_>>().join(", ");
    if paths.len() > 5 {
        shown.push_str(&format!(", +{} more", paths.len() - 5));
    }
    GuardrailResult {
        name: "protected files intact".into(),
        passed: false,
        detail: Some(format!(
            "trusted files were modified by the candidate's setup/tests/bench and restored from bench.ref: {shown}"
        )),
    }
}

pub async fn run_pipeline(ctx: PipelineContext<'_>) -> BenchResult<PipelineResult> {
    let PipelineContext {
        job,
        config,
        baseline,
        exec,
        log,
        setup,
        timeouts,
        command_guardrails_in_confirm,
        cancel,
        on_progress,
    } = ctx;
    let mut guardrails: Vec<GuardrailResult> = Vec::new();
    let mut runs: Vec<BenchRun> = Vec::new();
    let phase = phase_str(job.phase);
    let mut base_env = indexmap::IndexMap::new();
    base_env.insert("RATCHET_PHASE".to_owned(), phase.to_owned());
    base_env.insert("RATCHET_SHA".to_owned(), job.sha.clone());
    base_env.insert("RATCHET_OPTIMIZATION".to_owned(), job.optimization_id.to_string());
    base_env.insert("RATCHET_REPEATS".to_owned(), job.repeats.to_string());
    let emit = |p: BenchProgress| {
        if let Some(f) = on_progress {
            f(p);
        }
    };

    let trace = if job.phase == BenchJobPhase::Confirm {
        job.trace.clone().or_else(|| config_trace(config))
    } else {
        None
    };
    if job.phase == BenchJobPhase::Confirm && trace.is_none() {
        return Err(BenchError::infra(
            "confirm job has no trace: set job.trace or bench.confirm_trace in ratchet.yaml",
        ));
    }
    if job.repeats < 1 {
        return Err(BenchError::infra(format!(
            "job.repeats must be a positive integer (got {})",
            job.repeats
        )));
    }

    throw_if_aborted(cancel)?;
    emit(BenchProgress::Checkout);
    log.section(&format!(
        "checkout {} (bench from {})",
        job.sha, config.evaluator.git_ref
    ));
    if let Some(veto) = exec.checkout(trace.as_deref()).await? {
        log.line(&format!(
            "blocked before running anything: {}",
            veto.guardrail
                .detail
                .clone()
                .unwrap_or_else(|| veto.guardrail.name.clone())
        ));
        return Ok(PipelineResult {
            runs,
            guardrails: vec![veto.guardrail],
        });
    }

    macro_rules! guarded {
        ($spec:expr) => {{
            throw_if_aborted(cancel)?;
            let out = exec.run_step(&$spec).await?;
            match &out.tampered {
                Some(t) if !t.is_empty() => {
                    guardrails.push(tampered_guardrail(t));
                    None
                }
                _ => Some(out),
            }
        }};
    }

    if let Some(setup_cmd) = setup {
        emit(BenchProgress::Setup);
        let spec = StepSpec {
            kind: StepKind::Setup,
            name: "setup".into(),
            command: setup_cmd.to_owned(),
            timeout_ms: timeouts.setup_ms,
            env: base_env.clone(),
            trace: None,
        };
        let Some(out) = guarded!(spec) else {
            return Ok(PipelineResult { runs, guardrails });
        };
        let failed = out.timed_out || out.error.is_some() || out.exit_code != Some(0);
        if failed {
            let why = if out.timed_out {
                format!("timed out after {}ms", timeouts.setup_ms)
            } else if let Some(e) = &out.error {
                e.clone()
            } else {
                match out.exit_code {
                    Some(c) => format!("exited with status {c}"),
                    None => "exited with status null".to_owned(),
                }
            };
            let detail = match exec.setup_failure_hint() {
                Some(hint) => format!("{why} ({hint})"),
                None => why,
            };
            guardrails.push(GuardrailResult {
                name: "setup".into(),
                passed: false,
                detail: Some(detail),
            });
            return Ok(PipelineResult { runs, guardrails });
        }
    }

    if job.phase != BenchJobPhase::Confirm || command_guardrails_in_confirm {
        let commands: Vec<&CommandGuardrail> = config
            .guardrails
            .iter()
            .filter_map(|g| match g {
                Guardrail::Command(c) => Some(c),
                Guardrail::Threshold(_) => None,
            })
            .collect();
        let total = commands.len();
        for (i, g) in commands.iter().enumerate() {
            emit(BenchProgress::Guardrails {
                index: i + 1,
                total,
                name: g.run.clone(),
            });
            let spec = StepSpec {
                kind: StepKind::Guardrail,
                name: format!("guardrail {}/{}: {}", i + 1, total, g.run),
                command: g.run.clone(),
                timeout_ms: timeouts.guardrail_ms,
                env: base_env.clone(),
                trace: None,
            };
            let Some(out) = guarded!(spec) else {
                return Ok(PipelineResult { runs, guardrails });
            };
            let res = command_guardrail_result(g, &out, timeouts.guardrail_ms);
            let passed = res.passed;
            let detail = res.detail.clone();
            guardrails.push(res);
            if !passed {
                log.line(&format!(
                    "guardrail failed ({}); the benchmark will not run",
                    detail.unwrap_or_else(|| "failed".into())
                ));
                return Ok(PipelineResult { runs, guardrails }); // fail fast: blocked before spending bench time
            }
        }
    }

    let mut bench_failed = false;
    for i in 1..=job.repeats {
        emit(BenchProgress::Bench {
            run: i,
            of: job.repeats,
            phase: job.phase,
        });
        let mut env = base_env.clone();
        env.insert("RATCHET_RUN".to_owned(), i.to_string());
        let spec = StepSpec {
            kind: StepKind::Bench,
            name: format!("bench run {i}/{}", job.repeats),
            command: config.evaluator.run.clone(),
            timeout_ms: timeouts.bench_ms,
            env,
            trace: trace.clone(),
        };
        let Some(out) = guarded!(spec) else {
            return Ok(PipelineResult { runs, guardrails });
        };
        let mut fail = |detail: String| {
            guardrails.push(GuardrailResult {
                name: "bench.run".into(),
                passed: false,
                detail: Some(format!("run {i}/{}: {detail}", job.repeats)),
            });
            bench_failed = true;
        };
        if out.timed_out {
            fail(format!("timed out after {}ms", timeouts.bench_ms));
        } else if let Some(e) = &out.error {
            fail(e.clone());
        } else if out.exit_code != Some(0) {
            fail(match out.exit_code {
                None => "terminated by a signal".to_owned(),
                Some(c) => format!("exited with status {c}"),
            });
        } else {
            match parse_bench_output(&out.stdout, &config.objective.metric) {
                Err(e) => fail(e),
                Ok(parsed) => runs.push(BenchRun {
                    value: parsed.value,
                    metrics: parsed.metrics,
                    duration_ms: out.duration_ms.round().max(0.0) as u64,
                }),
            }
        }
        if bench_failed {
            break;
        }
    }

    if !bench_failed {
        emit(BenchProgress::Evaluate);
        let thresholds: Vec<ThresholdGuardrail> = config
            .guardrails
            .iter()
            .filter_map(|g| match g {
                Guardrail::Threshold(t) => Some(t.clone()),
                Guardrail::Command(_) => None,
            })
            .collect();
        guardrails.extend(evaluate_thresholds(&thresholds, &runs, baseline, job.phase));
    }
    Ok(PipelineResult { runs, guardrails })
}

/// Log key: `bench/<optimization>/<candidate|baseline>/<phase>.log`.
pub fn log_key(job: &BenchJob) -> String {
    let seg = |s: &str| -> String {
        let mut out = String::new();
        for c in s.chars() {
            if c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-' {
                out.push(c);
            } else {
                for _ in 0..c.len_utf16() {
                    out.push('_');
                }
            }
        }
        out
    };
    let who = if job.phase == BenchJobPhase::Baseline {
        "baseline".to_owned()
    } else {
        seg(job
            .candidate_id
            .as_ref()
            .map_or(job.sha.as_str(), |c| c.as_str()))
    };
    format!(
        "bench/{}/{who}/{}.log",
        seg(job.optimization_id.as_str()),
        phase_str(job.phase)
    )
}

/// Race `fut` against a timeout and a cancellation token (for calls that cannot be cancelled, like `GitHost::checkout`).
pub async fn with_limits<T>(
    fut: impl Future<Output = T>,
    what: &str,
    timeout_ms: u64,
    cancel: Option<&CancellationToken>,
) -> BenchResult<T> {
    let cancelled = async {
        match cancel {
            Some(c) => c.cancelled().await,
            None => std::future::pending::<()>().await,
        }
    };
    tokio::select! {
        v = fut => Ok(v),
        () = tokio::time::sleep(Duration::from_millis(timeout_ms)) => {
            Err(BenchError::infra(format!("{what} timed out after {timeout_ms}ms")))
        }
        () = cancelled => Err(BenchError::Aborted),
    }
}

pub struct JobDeps {
    pub clock: Arc<dyn Clock>,
    pub artifacts: Option<Arc<dyn ArtifactStore>>,
    pub setup: Option<String>,
    pub timeouts: StepTimeouts,
    pub command_guardrails_in_confirm: bool,
}

fn error_line(err: &BenchError) -> String {
    match err {
        BenchError::Infra(m) => format!("BenchError: {m}"),
        BenchError::Aborted => "AbortError: bench job aborted".into(),
        BenchError::Cancelled => "AbortError: bench job cancelled".into(),
    }
}

/// Run one job end to end: build the executor, run the pipeline, always dispose the executor and (best effort) store the
/// combined log, then assemble the `BenchOutcome`. Fails with `Aborted` when `cancel` fires and `Infra` for infrastructure
/// failures.
pub async fn run_bench_job<F, Fut>(
    deps: &JobDeps,
    job: &BenchJob,
    config: &RatchetConfig,
    baseline: Option<&BaselineMetrics>,
    cancel: Option<&CancellationToken>,
    on_progress: Option<ProgressFn<'_>>,
    make_executor: F,
) -> BenchResult<BenchOutcome>
where
    F: FnOnce(BenchLog) -> Fut,
    Fut: Future<Output = BenchResult<Box<dyn PipelineExecutor>>> + Send,
{
    let started_at = deps.clock.now();
    let log = BenchLog::new(deps.clock.clone());
    log.line(&format!(
        "bench job: optimization={} phase={} sha={} repeats={}{}",
        job.optimization_id,
        phase_str(job.phase),
        job.sha,
        job.repeats,
        job.candidate_id
            .as_ref()
            .map(|c| format!(" candidate={c}"))
            .unwrap_or_default()
    ));
    let mut exec: Option<Box<dyn PipelineExecutor>> = None;
    let mut result: Option<PipelineResult> = None;
    let mut failure: Option<BenchError> = None;
    let outcome: BenchResult<()> = async {
        throw_if_aborted(cancel)?;
        let e = make_executor(log.clone()).await?;
        exec = Some(e);
        let exec_ref = exec.as_mut().map(|b| &mut **b).ok_or_else(|| BenchError::infra("no executor"))?;
        result = Some(
            run_pipeline(PipelineContext {
                job,
                config,
                baseline,
                exec: exec_ref,
                log: &log,
                setup: deps.setup.as_deref(),
                timeouts: deps.timeouts,
                command_guardrails_in_confirm: deps.command_guardrails_in_confirm,
                cancel,
                on_progress,
            })
            .await?,
        );
        Ok(())
    }
    .await;
    if let Err(err) = outcome {
        log.line(&format!("\n!!! {}", error_line(&err)));
        failure = Some(err);
    }
    if let Some(e) = exec.as_mut() {
        e.dispose().await;
    }
    let mut logs_key: Option<String> = None;
    if let Some(store) = &deps.artifacts {
        let key = log_key(job);
        if store.put(&key, log.to_string().as_bytes()).await.is_ok() {
            logs_key = Some(key); // logs are best effort
        }
    }
    if let Some(err) = failure {
        return Err(err);
    }
    let Some(result) = result else {
        return Err(BenchError::infra("bench pipeline produced no result"));
    };
    Ok(BenchOutcome {
        samples: result.runs.iter().map(|r| r.value).collect(),
        runs: result.runs,
        guardrails: result.guardrails,
        logs_key,
        started_at,
        finished_at: deps.clock.now(),
    })
}
