//! Trusted-bench pipeline on this machine (simulation mode and `ratchet bench`). Port of `local.ts`. NOT a security boundary:
//! candidate code runs as the current user with the current environment.
//!
//! Per job: fresh directory under `scratch_root` with `code/` (job.sha) and `bench/` (config.bench.ref); `.git` is removed
//! from both; every file of bench.ref matching a `protected` glob overwrites the candidate's (see `overlay`); then setup,
//! command guardrails, `bench.run` x `job.repeats`, threshold guardrails (see `pipeline`). Trusted files are re-verified
//! before every step.
//!
//! Step environment: `RATCHET_PHASE`, `RATCHET_SHA`, `RATCHET_OPTIMIZATION`, `RATCHET_REPEATS`, `RATCHET_RUN` (bench runs,
//! 1-based) and, for phase `confirm` bench runs only, `RATCHET_TRACE`: the hidden trace (`job.trace`, else
//! `bench.confirm_trace`). A relative local path is made absolute; URLs (`s3://...`) are passed through unchanged, so the
//! bench must be able to read them.

use std::path::{Path, PathBuf};
use std::sync::{Arc, LazyLock};

use async_trait::async_trait;
use indexmap::IndexMap;
use ratchet_core::ports::{
    ArtifactStore, BaselineMetrics, BenchJob, BenchOutcome, BenchRunner, CancellationToken, Clock,
    GitHost, PortResult,
};
use ratchet_core::{ProtectedConfigInput, RatchetConfig, effective_protected};
use regex::Regex;

use crate::deps::DepsStrategyId;
use crate::deps::{
    dependency_change_guardrail, hash_named_files, lockfile_changes, pinned_deps_files,
    resolve_deps_strategy, root_entries,
};
use crate::errors::{BenchError, BenchResult};
use crate::exec::{OnData, ProcessOptions, run_process, signal_name};
use crate::log::BenchLog;
use crate::node_path::{NodeRuntime, WithNodeOnPathOptions, with_node_on_path};
use crate::overlay::{apply_overlay, is_protected, restore_protected};
use crate::pipeline::{
    CheckoutVeto, DEFAULT_TIMEOUTS, JobDeps, PipelineExecutor, ProgressBenchRunner, ProgressFn,
    StepOutcome, StepSpec, StepTimeouts, run_bench_job, with_limits,
};

#[derive(Clone)]
pub struct LocalBenchRunnerOptions {
    /// Used to materialize `job.sha` and `config.bench.ref` (any ref `GitHost::checkout` can resolve).
    pub git: Arc<dyn GitHost>,
    /// Fresh job directories are created under here and deleted afterwards.
    pub scratch_root: PathBuf,
    pub clock: Arc<dyn Clock>,
    /// Run in the candidate checkout before the guardrails, e.g. `pnpm install --frozen-lockfile && pnpm build`. When it is
    /// set, a candidate whose lockfile differs from `bench.ref`'s is BLOCKED exactly as on the real bench (which has no
    /// network, so a dependency change cannot be installed there), and the lockfile counts as protected. This machine has a
    /// network and needs no snapshot; the rule is applied for parity, so a simulation does not accept what live mode would
    /// refuse.
    pub setup: Option<String>,
    /// Combined logs go to `bench/<optimization>/<candidate|baseline>/<phase>.log`.
    pub artifacts: Option<Arc<dyn ArtifactStore>>,
    pub timeouts: StepTimeouts,
    /// Extra environment for every step (merged over this process's environment).
    pub env: IndexMap<String, String>,
    /// How `node` is made to resolve for every step (see `node_path`). Default: the first `node` on PATH.
    pub node_runtime: NodeRuntime,
    /// Run command guardrails in the confirm phase too. Default false (they passed in the bench phase).
    pub command_guardrails_in_confirm: bool,
    /// Delete candidate files that match `protected` but are absent from bench.ref. Default true.
    pub prune_extra_protected: bool,
    /// Keep the job directory (debugging). Default false.
    pub keep_workdir: bool,
}

impl LocalBenchRunnerOptions {
    pub fn new(
        git: Arc<dyn GitHost>,
        scratch_root: impl Into<PathBuf>,
        clock: Arc<dyn Clock>,
    ) -> Self {
        LocalBenchRunnerOptions {
            git,
            scratch_root: scratch_root.into(),
            clock,
            setup: None,
            artifacts: None,
            timeouts: DEFAULT_TIMEOUTS,
            env: IndexMap::new(),
            node_runtime: NodeRuntime::Current,
            command_guardrails_in_confirm: false,
            prune_extra_protected: true,
            keep_workdir: false,
        }
    }
}

static URL_LIKE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?i)^[a-z][a-z0-9+.-]*://").expect("static regex"));

pub struct LocalBenchRunner {
    o: LocalBenchRunnerOptions,
}

impl LocalBenchRunner {
    pub fn new(o: LocalBenchRunnerOptions) -> Self {
        LocalBenchRunner { o }
    }
}

#[async_trait]
impl BenchRunner for LocalBenchRunner {
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
impl ProgressBenchRunner for LocalBenchRunner {
    async fn run_with_progress(
        &self,
        job: &BenchJob,
        config: &RatchetConfig,
        baseline: Option<&BaselineMetrics>,
        cancel: Option<&CancellationToken>,
        on_progress: Option<ProgressFn<'_>>,
    ) -> BenchResult<BenchOutcome> {
        let o = &self.o;
        run_bench_job(
            &JobDeps {
                clock: o.clock.clone(),
                artifacts: o.artifacts.clone(),
                setup: o.setup.clone(),
                timeouts: o.timeouts,
                command_guardrails_in_confirm: o.command_guardrails_in_confirm,
            },
            job,
            config,
            baseline,
            cancel,
            on_progress,
            |log| {
                let o = o.clone();
                let job = job.clone();
                let config = config.clone();
                let cancel = cancel.cloned();
                async move {
                    tokio::fs::create_dir_all(&o.scratch_root)
                        .await
                        .map_err(|e| {
                            BenchError::infra(format!("{}: {e}", o.scratch_root.display()))
                        })?;
                    let prefix: String = job
                        .optimization_id
                        .as_str()
                        .chars()
                        .map(|c| {
                            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                                c
                            } else {
                                '_'
                            }
                        })
                        .collect();
                    let root = tempfile::Builder::new()
                        .prefix(&format!("{prefix}-"))
                        .tempdir_in(&o.scratch_root)
                        .map_err(|e| {
                            BenchError::infra(format!("cannot create the job directory: {e}"))
                        })?
                        .keep();
                    let exec: Box<dyn PipelineExecutor> =
                        Box::new(LocalExecutor::new(o, job, config, log, root, cancel));
                    Ok(exec)
                }
            },
        )
        .await
    }
}

struct LocalExecutor {
    o: LocalBenchRunnerOptions,
    job: BenchJob,
    config: RatchetConfig,
    log: BenchLog,
    root: PathBuf,
    code_dir: PathBuf,
    bench_dir: PathBuf,
    cancel: Option<CancellationToken>,
    protected_files: Vec<String>,
    timeouts: StepTimeouts,
}

/// Run blocking filesystem work off the async workers.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> BenchResult<T> + Send + 'static,
) -> BenchResult<T> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| BenchError::infra(format!("filesystem task failed: {e}")))?
}

impl LocalExecutor {
    fn new(
        o: LocalBenchRunnerOptions,
        job: BenchJob,
        config: RatchetConfig,
        log: BenchLog,
        root: PathBuf,
        cancel: Option<CancellationToken>,
    ) -> Self {
        let timeouts = o.timeouts;
        LocalExecutor {
            code_dir: root.join("code"),
            bench_dir: root.join("bench"),
            o,
            job,
            config,
            log,
            root,
            cancel,
            protected_files: Vec::new(),
            timeouts,
        }
    }

    async fn checkout_ref(&self, git_ref: &str, dir: &Path, what: String) -> BenchResult<()> {
        let r = with_limits(
            self.o.git.checkout(git_ref, dir),
            &what,
            self.timeouts.checkout_ms,
            self.cancel.as_ref(),
        )
        .await?;
        r.map_err(|e| {
            let msg = e.to_string();
            BenchError::infra(format!(
                "{what} failed: {}",
                msg.split('\n').next().unwrap_or("")
            ))
        })
    }
}

#[async_trait]
impl PipelineExecutor for LocalExecutor {
    async fn checkout(&mut self, _trace: Option<&str>) -> BenchResult<Option<CheckoutVeto>> {
        for d in [&self.code_dir, &self.bench_dir] {
            tokio::fs::create_dir(d)
                .await
                .map_err(|e| BenchError::infra(format!("{}: {e}", d.display())))?;
        }
        let bench_ref = self.config.evaluator.git_ref.clone();
        self.checkout_ref(
            &self.job.sha,
            &self.code_dir,
            format!("checkout of candidate {}", self.job.sha),
        )
        .await?;
        self.checkout_ref(
            &bench_ref,
            &self.bench_dir,
            format!("checkout of bench.ref {bench_ref}"),
        )
        .await?;
        for d in [&self.code_dir, &self.bench_dir] {
            let _ = tokio::fs::remove_dir_all(d.join(".git")).await;
        }
        // Dependencies cannot change in v0 (the real bench is offline). Decided before anything runs, from the trees as
        // checked out.
        let mut pinned: Vec<String> = Vec::new();
        if self.o.setup.is_some() {
            let bench_dir = self.bench_dir.clone();
            let code_dir = self.code_dir.clone();
            let mode = self.config.evaluator.deps;
            let (resolved, changed) = blocking(move || {
                let resolved = resolve_deps_strategy(mode, root_entries(&bench_dir)?)?;
                let mut changed = Vec::new();
                if resolved.strategy.id != DepsStrategyId::None {
                    let pinned = pinned_deps_files(&resolved.strategy);
                    changed = lockfile_changes(
                        &hash_named_files(&bench_dir, &pinned)?,
                        &hash_named_files(&code_dir, &pinned)?,
                    );
                }
                Ok((resolved, changed))
            })
            .await?;
            if let Some(note) = &resolved.note {
                self.log.line(&format!("dependencies: {note}"));
            }
            if resolved.strategy.id != DepsStrategyId::None {
                pinned = pinned_deps_files(&resolved.strategy);
                if !changed.is_empty() {
                    self.log.line(&format!(
                        "!!! dependency change: {} differs from bench.ref {bench_ref}",
                        changed.join(", ")
                    ));
                    return Ok(Some(CheckoutVeto {
                        guardrail: dependency_change_guardrail(&changed, &bench_ref),
                    }));
                }
            }
        }
        // Evaluator, config, environment and the user's list (lockfiles are pinned through the dependency strategy above).
        let eff = effective_protected(&ProtectedConfigInput::from(&self.config), None);
        let mut globs = eff.evaluator_globs.clone();
        globs.extend(pinned);
        let (code_dir, bench_dir, prune) = (
            self.code_dir.clone(),
            self.bench_dir.clone(),
            self.o.prune_extra_protected,
        );
        let res = blocking(move || apply_overlay(&code_dir, &bench_dir, &globs, prune)).await?;
        if !res.copied.iter().any(|p| is_protected(p, &eff.required)) {
            return Err(BenchError::infra(format!(
                "bench.ref {bench_ref} contains no files matching the protected globs [{}]; refusing to benchmark with an unprotected bench",
                eff.required.join(", ")
            )));
        }
        self.log.line(&format!(
            "overlay: {} protected file(s) taken from {bench_ref}, {} candidate-only protected file(s) removed",
            res.copied.len(),
            res.removed.len()
        ));
        for p in &res.removed {
            self.log.line(&format!("  removed {p}"));
        }
        self.protected_files = res.copied;
        Ok(None)
    }

    async fn run_step(&mut self, spec: &StepSpec) -> BenchResult<StepOutcome> {
        let (code_dir, bench_dir, files) = (
            self.code_dir.clone(),
            self.bench_dir.clone(),
            self.protected_files.clone(),
        );
        let restored = blocking(move || restore_protected(&code_dir, &bench_dir, &files)).await?;
        if !restored.is_empty() {
            self.log.line(&format!(
                "!!! protected files were modified and restored: {}",
                restored.join(", ")
            ));
            return Ok(StepOutcome {
                tampered: Some(restored),
                ..StepOutcome::default()
            });
        }
        self.log.section(&spec.name);
        let mut env: IndexMap<String, String> = self.o.env.clone();
        for (k, v) in &spec.env {
            env.insert(k.clone(), v.clone());
        }
        let env = match &self.o.node_runtime {
            NodeRuntime::Disabled => env,
            NodeRuntime::Current => with_node_on_path(&env, &WithNodeOnPathOptions::default()),
            NodeRuntime::Explicit(info) => with_node_on_path(
                &env,
                &WithNodeOnPathOptions {
                    runtime: Some(info.clone()),
                    ..WithNodeOnPathOptions::default()
                },
            ),
        };
        let mut env: Vec<(String, String)> = env.into_iter().collect();
        if let Some(trace) = &spec.trace {
            let value = if URL_LIKE.is_match(trace) || Path::new(trace).is_absolute() {
                trace.clone()
            } else {
                std::env::current_dir()
                    .map(|d| d.join(trace).to_string_lossy().into_owned())
                    .unwrap_or_else(|_| trace.clone())
            };
            env.push(("RATCHET_TRACE".into(), value));
        }
        let log = self.log.clone();
        let on_data: OnData = Arc::new(move |chunk, _| log.write(chunk));
        let r = run_process(
            &spec.command,
            &ProcessOptions {
                cwd: self.code_dir.clone(),
                env,
                timeout_ms: Some(spec.timeout_ms),
                cancel: self.cancel.clone(),
                on_data: Some(on_data),
                ..ProcessOptions::new(&self.code_dir)
            },
        )
        .await;
        if r.aborted {
            return Err(BenchError::Aborted);
        }
        self.log.line(&match &r.spawn_error {
            Some(e) => format!("[could not start: {e}]"),
            None => format!(
                "[exit {}{} after {}ms]",
                r.exit_code.map_or_else(
                    || format!(
                        "signal {}",
                        r.signal.map_or_else(|| "null".into(), signal_name)
                    ),
                    |c| c.to_string()
                ),
                if r.timed_out { ", timed out" } else { "" },
                r.duration_ms.round() as i64
            ),
        });
        Ok(StepOutcome {
            exit_code: r.exit_code,
            timed_out: r.timed_out,
            stdout: r.stdout,
            duration_ms: r.duration_ms,
            tampered: None,
            error: r.spawn_error.map(|e| format!("could not start: {e}")),
        })
    }

    async fn dispose(&mut self) {
        if !self.o.keep_workdir {
            let _ = tokio::fs::remove_dir_all(&self.root).await;
        }
    }
}
