//! Live-mode bench runner (port of `remote.ts`). Per job it launches a fresh bench instance (so a confirm run is on a fresh
//! instance by construction), runs the same pipeline as `LocalBenchRunner` by sending one generated script per step through
//! `CloudCompute::run_command`, and ALWAYS terminates the instance. See `remote_scripts` for the scripts and the sentinel
//! protocol.
//!
//! Steps: [dependency snapshot, first job per lockfile only: a separate `deps` instance with egress builds the package store
//! from bench.ref's lockfile; see `remote_deps`] -> stage on the controller (candidate + bench.ref tarballs into the
//! artifacts bucket; a changed lockfile blocks the candidate here) -> ping (wait for SSM) -> checkout (download from S3,
//! verify digest, overlay, check the isolation, restore the snapshot read-only) -> setup (offline) -> guardrails (both as the
//! candidate user) -> bench x N (the harness as the holdout user). For a confirm job the sealed holdout is fetched by root
//! inside each bench run, after the build is over, and wiped when the run ends: nothing candidate-controlled ever runs while
//! it exists as a file the candidate user could open. Aborting stops waiting immediately and terminates the instance, which
//! kills whatever is running there.

use std::sync::Arc;

use async_trait::async_trait;
use base64::Engine as _;
use indexmap::IndexMap;
use ratchet_core::ports::{
    ArtifactStore, BaselineMetrics, BenchJob, BenchJobPhase, BenchLaunchPurpose, BenchLaunchSpec, BenchOutcome,
    BenchRunner, CancellationToken, Clock, CloudCompute, GitHost, InstanceInfo, PortResult,
};
use ratchet_core::{ProtectedConfigInput, RatchetConfig, effective_protected};

use crate::deps::{
    DepsPlan, dependency_change_guardrail, lockfile_changes, offline_setup_hint, pinned_deps_files,
};
use crate::errors::{BenchError, BenchResult};
use crate::image::{
    ImagePlan, container_setup_hint, environment_change_guardrail, environment_changes, environment_files_of,
    read_environment,
};
use crate::image_scripts::{ContainerOptions, ContainerRuntime, ImageRestore};
use crate::log::BenchLog;
use crate::pipeline::{
    CheckoutVeto, DEFAULT_TIMEOUTS, JobDeps, PipelineExecutor, ProgressBenchRunner, ProgressFn,
    StepOutcome, StepSpec, StepTimeouts, run_bench_job,
};
use crate::remote_deps::{DepsSnapshotOptions, DepsSnapshots, EnsureInput, PreparedEnvironment};
use crate::remote_scripts::{
    CheckoutScriptInput, DepsRestore, HOLDOUT_FETCH_EXIT, Isolation, Layout, StepScriptInput, checkout_script,
    deps_paths, parse_sentinels, step_script,
};
use crate::remote_session::{InstanceSession, SessionOptions, WarningSink, tail600};
use crate::stage::{StageBase, StageInput, StagedSource, stage_source};

#[derive(Clone)]
pub struct RemoteBenchRunnerOptions {
    pub compute: Arc<dyn CloudCompute>,
    /// Used on the controller to materialize the candidate and `bench.ref` before the job ("pre-staging"): bench instances
    /// have no route to GitHub and never fetch from it.
    pub git: Arc<dyn GitHost>,
    pub clock: Arc<dyn Clock>,
    /// AMI / image reference of the bench instance (tar, GNU coreutils + findutils + grep -P, bash, setpriv, iptables; `aws`
    /// CLI for s3:// downloads; see docs/real-env/bench.md).
    pub image_ref: String,
    /// Name of the artifacts bucket (`artifacts_bucket` output): staged inputs are uploaded under `bench-inputs/` and
    /// downloaded from there by the instance. Required to run a job.
    pub staging_bucket: Option<String>,
    /// Where staged inputs are written. Default: `artifacts`, which must then write to `staging_bucket`.
    pub staging_store: Option<Arc<dyn ArtifactStore>>,
    /// `User` (default): setup and guardrails run as the unprivileged candidate user (`ratchet-bench`); each bench run
    /// executes the evaluator harness as a second unprivileged user (`ratchet-holdout`) from a read-only snapshot of the
    /// built tree, and the harness starts candidate processes through the root-installed launcher. Instance metadata is
    /// blocked for both users, and the job is refused if the image lacks the users, the block, the launcher or the holdout
    /// directory permissions. `None` runs everything as root and is only for tests that execute the scripts on a developer
    /// machine.
    pub isolation: Isolation,
    /// Path of the candidate launcher on the image. Default `/usr/local/bin/ratchet-candidate`. Test seam.
    pub candidate_launcher: Option<String>,
    /// Candidate setup, e.g. `pnpm install --frozen-lockfile && pnpm build`. The bench has no network, so when it is set the
    /// dependencies come from a SNAPSHOT of the lockfile at `bench.ref` (see `deps` and `evaluator.deps`): built once per
    /// lockfile by a `deps` instance with egress before the first job, restored read-only before setup, and setup runs with
    /// the package manager offline. A candidate whose lockfile differs is blocked. Without `setup` nothing is snapshotted.
    pub setup: Option<String>,
    pub artifacts: Option<Arc<dyn ArtifactStore>>,
    /// Options of the repo's container when `environment.image` is set: setup, guardrails and the evaluator then run in it
    /// with `--network none` (see `image_scripts` for the isolation mapping). Without `environment.image` the steps run
    /// directly on the instance.
    pub container: Option<ContainerOptions>,
    /// The bench instance type. Default: `config.evaluator.hardware`.
    pub instance_type: Option<String>,
    /// Instance type of the dependency-snapshot job (must have the bench's architecture). Default: the bench's.
    pub deps_instance_type: Option<String>,
    /// Per-job working directories are created under here on the instance. Default `/var/tmp/ratchet-bench`.
    pub work_root: Option<String>,
    pub timeouts: StepTimeouts,
    pub command_guardrails_in_confirm: bool,
    pub prune_extra_protected: bool,
    /// The first command is retried while the instance's SSM agent registers. Defaults: 30 attempts, 10 s apart.
    pub ready_attempts: Option<u32>,
    pub ready_delay_ms: Option<u64>,
    /// Called with non-fatal problems, e.g. a failed `terminate` (the reaper is the backstop).
    pub on_warning: Option<WarningSink>,
}

impl RemoteBenchRunnerOptions {
    pub fn new(
        compute: Arc<dyn CloudCompute>,
        git: Arc<dyn GitHost>,
        clock: Arc<dyn Clock>,
        image_ref: impl Into<String>,
    ) -> Self {
        RemoteBenchRunnerOptions {
            compute,
            git,
            clock,
            image_ref: image_ref.into(),
            staging_bucket: None,
            staging_store: None,
            isolation: Isolation::User,
            candidate_launcher: None,
            setup: None,
            artifacts: None,
            container: None,
            instance_type: None,
            deps_instance_type: None,
            work_root: None,
            timeouts: DEFAULT_TIMEOUTS,
            command_guardrails_in_confirm: false,
            prune_extra_protected: true,
            ready_attempts: None,
            ready_delay_ms: None,
            on_warning: None,
        }
    }

    fn work_root(&self) -> String {
        self.work_root
            .clone()
            .unwrap_or_else(|| "/var/tmp/ratchet-bench".into())
    }

    fn session(&self) -> SessionOptions {
        SessionOptions {
            compute: self.compute.clone(),
            clock: self.clock.clone(),
            ready_attempts: self.ready_attempts,
            ready_delay_ms: self.ready_delay_ms,
            on_warning: self.on_warning.clone(),
        }
    }
}

pub struct RemoteBenchRunner {
    o: RemoteBenchRunnerOptions,
    snapshots: Arc<DepsSnapshots>,
}

impl RemoteBenchRunner {
    pub fn new(o: RemoteBenchRunnerOptions) -> Self {
        let snapshots = Arc::new(DepsSnapshots::new(DepsSnapshotOptions {
            session: o.session(),
            git: o.git.clone(),
            image_ref: o.image_ref.clone(),
            bucket: o.staging_bucket.clone(),
            store: o.staging_store.clone().or_else(|| o.artifacts.clone()),
            isolation: o.isolation,
            work_root: o.work_root(),
            deps_ms: o.timeouts.deps_ms,
            image_ms: Some(o.timeouts.image_ms),
            container: o.container.clone(),
            bench_instance_type: o.instance_type.clone(),
            instance_type: o.deps_instance_type.clone(),
        }));
        RemoteBenchRunner { o, snapshots }
    }
}

#[async_trait]
impl BenchRunner for RemoteBenchRunner {
    async fn run(
        &self,
        job: &BenchJob,
        config: &RatchetConfig,
        baseline: Option<&BaselineMetrics>,
        cancel: Option<&CancellationToken>,
    ) -> PortResult<BenchOutcome> {
        Ok(self.run_with_progress(job, config, baseline, cancel, None).await?)
    }
}

#[async_trait]
impl ProgressBenchRunner for RemoteBenchRunner {
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
            |log| async move {
                // Before the first job of a lockfile / Dockerfile (the baseline, in practice) a separate instance with egress
                // fetches the packages and builds the repo's container image; it is gone again before this job's instance
                // exists. Without setup there is nothing to install, and without `environment.image` there is no container.
                let wants_image = read_environment(config)?.image.is_some();
                let prepared: PreparedEnvironment = if o.setup.is_some() || wants_image {
                    self.snapshots
                        .prepare(
                            &EnsureInput {
                                job,
                                config,
                                log: &log,
                                cancel,
                                on_progress,
                            },
                            o.setup.is_some(),
                        )
                        .await?
                } else {
                    PreparedEnvironment::default()
                };
                let instance = o
                    .compute
                    .launch_bench_instance(&BenchLaunchSpec {
                        optimization_id: job.optimization_id.clone(),
                        instance_type: o
                            .instance_type
                            .clone()
                            .unwrap_or_else(|| config.evaluator.hardware.clone()),
                        image_ref: o.image_ref.clone(),
                        purpose: match job.phase {
                            BenchJobPhase::Baseline => BenchLaunchPurpose::Baseline,
                            BenchJobPhase::Bench => BenchLaunchPurpose::Bench,
                            BenchJobPhase::Confirm => BenchLaunchPurpose::Confirm,
                        },
                    })
                    .await
                    .map_err(BenchError::from)?;
                log.line(&format!(
                    "bench instance {} ({}) launched for {}",
                    instance.id,
                    instance.instance_type,
                    crate::pipeline::phase_str(job.phase)
                ));
                let exec: Box<dyn PipelineExecutor> = Box::new(RemoteExecutor::new(
                    o.clone(),
                    job.clone(),
                    config.clone(),
                    log,
                    instance,
                    cancel.cloned(),
                    prepared,
                ));
                Ok(exec)
            },
        )
        .await
    }
}

struct RemoteExecutor {
    o: RemoteBenchRunnerOptions,
    job: BenchJob,
    config: RatchetConfig,
    log: BenchLog,
    layout: Layout,
    timeouts: StepTimeouts,
    session: InstanceSession,
    deps: Option<DepsPlan>,
    image: Option<ImagePlan>,
}

impl RemoteExecutor {
    fn new(
        o: RemoteBenchRunnerOptions,
        job: BenchJob,
        config: RatchetConfig,
        log: BenchLog,
        instance: InstanceInfo,
        cancel: Option<CancellationToken>,
        prepared: PreparedEnvironment,
    ) -> Self {
        let opt: String = job
            .optimization_id
            .as_str()
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '_' })
            .collect();
        let id = uuid::Uuid::new_v4().to_string();
        let layout = Layout::new(format!("{}/{opt}-{}", o.work_root(), &id[..8]));
        let timeouts = o.timeouts;
        let session = InstanceSession::new(o.session(), instance, log.clone(), cancel, "bench");
        RemoteExecutor {
            o,
            job,
            config,
            log,
            layout,
            timeouts,
            session,
            deps: prepared.deps,
            image: prepared.image,
        }
    }

    fn isolation(&self) -> Isolation {
        self.o.isolation
    }

    async fn stage(&self) -> BenchResult<StagedSource> {
        let store = self
            .o
            .staging_store
            .clone()
            .or_else(|| self.o.artifacts.clone());
        let (Some(bucket), Some(store)) = (self.o.staging_bucket.clone(), store) else {
            return Err(BenchError::infra(
                "RemoteBenchRunner needs stagingBucket (and an artifacts/stagingStore that writes to it): bench instances have no internet and cannot fetch the candidate themselves",
            ));
        };
        let image_path = self.image.as_ref().map(|i| i.spec.path.clone());
        let staged = stage_source(&StageInput {
            base: StageBase {
                git: self.o.git.clone(),
                store,
                bucket,
                optimization_id: self.job.optimization_id.to_string(),
            },
            sha: self.job.sha.clone(),
            // With a snapshot, the bench tree is the exact commit the snapshot's lockfile hashes were computed from.
            bench_ref: self
                .deps
                .as_ref()
                .map(|d| d.base_commit.clone())
                .or_else(|| self.image.as_ref().map(|i| i.base_commit.clone()))
                .unwrap_or_else(|| self.config.evaluator.git_ref.clone()),
            hash_files: self.deps.as_ref().map(|d| pinned_deps_files(&d.strategy)),
            hash_environment: image_path.map(|p| {
                Arc::new(move |tree: &std::path::Path| environment_files_of(tree, &p))
                    as crate::stage::EnvHasher
            }),
        })
        .await?;
        self.log.line(&format!(
            "staged candidate {} and bench.ref {} for the instance (no GitHub access from the bench)",
            staged.code.commit, staged.bench.commit
        ));
        Ok(staged)
    }

    /// What the checkout script restores before setup, from the manifest the snapshot job wrote.
    fn restore(&self, plan: &DepsPlan) -> DepsRestore {
        let bucket = self.o.staging_bucket.clone().unwrap_or_default();
        DepsRestore {
            strategy: plan.strategy.clone(),
            uri: format!("s3://{bucket}/{}", plan.manifest.archive),
            sha256: plan.manifest.sha256.clone(),
            compression: plan.manifest.compression,
            node: plan.manifest.node.clone(),
            pm: plan.manifest.pm.clone(),
        }
    }

    fn runtime(&self, plan: &ImagePlan) -> ContainerRuntime {
        self.o.container.clone().unwrap_or_default().runtime(plan.tag.clone())
    }

    /// The saved container image the checkout loads (offline) before setup.
    fn restore_image(&self, plan: &ImagePlan) -> ImageRestore {
        let bucket = self.o.staging_bucket.clone().unwrap_or_default();
        ImageRestore {
            uri: format!("s3://{bucket}/{}", plan.manifest.archive),
            sha256: plan.manifest.sha256.clone(),
            compression: plan.manifest.compression,
            image_id: plan.manifest.image_id.clone(),
            rt: self.runtime(plan),
        }
    }
}

#[async_trait]
impl PipelineExecutor for RemoteExecutor {
    async fn checkout(&mut self, trace: Option<&str>) -> BenchResult<Option<CheckoutVeto>> {
        // Staging overlaps with the instance booting (it was launched before checkout started).
        let staged = self.stage().await?;
        let bench_ref = self.config.evaluator.git_ref.clone();

        // Dependencies cannot change in v0: the bench has no network. Decided here, on the controller, from the trees it
        // just archived, so a candidate that touches the lockfile never gets as far as running anything.
        if let Some(deps) = &self.deps {
            let empty = crate::deps::FileHashes::new();
            let base = lockfile_changes(&deps.lockfiles, staged.bench.files.as_ref().unwrap_or(&empty));
            if !base.is_empty() {
                return Err(BenchError::infra(format!(
                    "bench.ref {bench_ref} moved while the job was starting: {} no longer matches the dependency snapshot {}",
                    base.join(", "),
                    deps.key
                )));
            }
            let changed = lockfile_changes(&deps.lockfiles, staged.code.files.as_ref().unwrap_or(&empty));
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

        // The same for the environment: the container is built once from bench.ref; a candidate that changes the Dockerfile /
        // devcontainer or a file it copies in is blocked, not built with network access.
        if let Some(image) = &self.image {
            let empty = crate::deps::FileHashes::new();
            let moved = environment_changes(&image.files, staged.bench.env.as_ref().unwrap_or(&empty));
            if !moved.is_empty() {
                return Err(BenchError::infra(format!(
                    "bench.ref {bench_ref} moved while the job was starting: {} no longer matches the container image {}",
                    moved.join(", "),
                    image.key
                )));
            }
            let changed = environment_changes(&image.files, staged.code.env.as_ref().unwrap_or(&empty));
            if !changed.is_empty() {
                self.log.line(&format!(
                    "!!! environment change: {} differs from bench.ref {bench_ref}",
                    changed.join(", ")
                ));
                return Ok(Some(CheckoutVeto {
                    guardrail: environment_change_guardrail(&changed, &bench_ref),
                }));
            }
        }

        // Evaluator, config, environment and the user's list (lockfiles are pinned through the dependency strategy above).
        let eff = effective_protected(&ProtectedConfigInput::from(&self.config), None);
        // `globs` count toward "the evaluator ref holds something protected"; the rest (ratchet.yaml, the environment) is
        // pinned like the lockfiles.
        let extra_pinned: Vec<String> = eff
            .evaluator_globs
            .iter()
            .filter(|g| !eff.required.contains(g))
            .cloned()
            .collect();
        let protected_globs = eff.required.clone();
        self.session.wait_ready().await?;
        let mut pinned: Vec<String> = self
            .deps
            .as_ref()
            .map(|d| pinned_deps_files(&d.strategy))
            .unwrap_or_default();
        pinned.extend(extra_pinned);
        if let Some(i) = &self.image {
            pinned.extend(i.pinned.iter().cloned());
        }
        let script = checkout_script(&CheckoutScriptInput {
            layout: self.layout.clone(),
            code: staged.code.clone(),
            bench: staged.bench.clone(),
            globs: protected_globs.clone(),
            pinned,
            prune: self.o.prune_extra_protected,
            isolation: self.isolation(),
            deps: self.deps.as_ref().map(|d| self.restore(d)),
            image: self.image.as_ref().map(|i| self.restore_image(i)),
            candidate_launcher: self.o.candidate_launcher.clone(),
        })?;
        let res = self
            .session
            .call("checkout", &script, self.timeouts.checkout_ms)
            .await?;
        let s = parse_sentinels(&res.stdout)
            .into_iter()
            .find(|x| x.kind == "checkout");
        let Some(s) = s.filter(|_| res.exit_code == 0) else {
            return Err(BenchError::infra(format!(
                "checkout on the bench instance failed (exit {}): {}",
                res.exit_code,
                tail600(if res.stderr.is_empty() { &res.stdout } else { &res.stderr })
            )));
        };
        let num = |k: &str| s.data.get(k).and_then(serde_json::Value::as_f64);
        let matched = num("matched").or_else(|| num("copied")).unwrap_or(f64::NAN);
        if !(matched > 0.0) {
            return Err(BenchError::infra(format!(
                "bench.ref {bench_ref} contains no files matching the protected globs [{}]; refusing to benchmark with an unprotected bench",
                protected_globs.join(", ")
            )));
        }
        let shown = |k: &str| s.data.get(k).map_or_else(|| "undefined".to_owned(), json_text);
        self.log.line(&format!(
            "overlay: {} protected file(s) from {bench_ref}, {} candidate-only protected file(s) removed",
            shown("copied"),
            shown("removed")
        ));
        if let Some(image) = &self.image {
            self.log.line(&format!(
                "container image {} loaded and probed (--network none, caps dropped, read-only root); setup, guardrails and the evaluator run in it",
                image.key
            ));
        }
        if let Some(deps) = &self.deps {
            self.log.line(&format!(
                "dependencies restored read-only from snapshot {}; setup runs offline",
                deps.key
            ));
        }

        // The holdout is NOT fetched here: setup and guardrails (candidate code) run before the first bench step, and
        // nothing sealed may exist on the instance while they do. Each bench step fetches it.
        if trace.is_some() {
            self.log.line("holdout is fetched by each bench run, after setup and guardrails, and wiped when the run ends");
        }
        Ok(None)
    }

    fn setup_failure_hint(&self) -> Option<String> {
        let hints: Vec<String> = [
            self.deps
                .as_ref()
                .map(|d| offline_setup_hint(&d.strategy, &self.config.evaluator.git_ref)),
            self.image
                .as_ref()
                .map(|_| container_setup_hint(&self.config.evaluator.git_ref)),
        ]
        .into_iter()
        .flatten()
        .collect();
        if hints.is_empty() {
            None
        } else {
            Some(hints.join("; "))
        }
    }

    async fn run_step(&mut self, spec: &StepSpec) -> BenchResult<StepOutcome> {
        self.log.section(&spec.name);
        // Setup and the command guardrails (candidate code, as the candidate user) install from the restored store with the
        // package manager offline. The evaluator harness does not.
        let mut env: IndexMap<String, String> = spec.env.clone();
        if let Some(deps) = &self.deps
            && spec.kind != crate::remote_scripts::StepKind::Bench
        {
            let (store, scratch) = deps_paths(&self.layout);
            for (k, v) in deps.strategy.setup_env(&store, &scratch) {
                env.insert(k, v);
            }
        }
        let mut input = StepScriptInput::new(
            self.layout.clone(),
            spec.kind,
            spec.name.clone(),
            spec.command.clone(),
            spec.timeout_ms,
        );
        input.env = env;
        input.isolation = self.isolation();
        input.holdout = spec.trace.clone();
        if let Some(image) = &self.image {
            input.container = Some(self.runtime(image));
            input.deps_mounted = self.deps.is_some();
        }
        input.candidate_launcher = self.o.candidate_launcher.clone();
        let script = step_script(&input)?;
        let res = self.session.call(&spec.name, &script, spec.timeout_ms).await?;
        let sentinels = parse_sentinels(&res.stdout);
        if spec.trace.is_some() && HOLDOUT_FETCH_EXIT.contains(&res.exit_code) {
            return Err(BenchError::infra(format!(
                "fetching the holdout on the bench instance failed (exit {}): {}",
                res.exit_code,
                tail600(if res.stderr.is_empty() { &res.stdout } else { &res.stderr })
            )));
        }
        if let Some(held) = sentinels.iter().find(|x| x.kind == "holdout") {
            self.log.line(&format!(
                "holdout: {} bytes fetched for this run, readable by the evaluator user only, wiped afterwards",
                held.data.get("bytes").map_or_else(|| "undefined".to_owned(), json_text)
            ));
        }
        if let Some(t) = sentinels.iter().find(|s| s.kind == "tampered") {
            let b64 = t.data.get("paths_b64").and_then(|v| v.as_str()).unwrap_or("");
            let decoded = base64::engine::general_purpose::STANDARD
                .decode(b64)
                .map(|b| String::from_utf8_lossy(&b).into_owned())
                .unwrap_or_default();
            let paths: Vec<String> = decoded
                .split('\n')
                .filter(|p| !p.is_empty())
                .map(str::to_owned)
                .collect();
            self.log.line(&format!(
                "!!! protected files were modified and restored: {}",
                paths.join(", ")
            ));
            return Ok(StepOutcome {
                tampered: Some(if paths.is_empty() {
                    vec!["(unknown)".into()]
                } else {
                    paths
                }),
                ..StepOutcome::default()
            });
        }
        let want = if spec.kind == crate::remote_scripts::StepKind::Bench {
            "run"
        } else {
            "step"
        };
        let Some(r) = sentinels.iter().find(|s| s.kind == want) else {
            return Ok(StepOutcome {
                error: Some(format!(
                    "no result from the bench instance (script exit {}): {}",
                    res.exit_code,
                    tail600(if res.stderr.is_empty() { &res.stdout } else { &res.stderr })
                )),
                ..StepOutcome::default()
            });
        };
        let exit_code = r
            .data
            .get("exit")
            .and_then(serde_json::Value::as_i64)
            .map(|c| c as i32);
        let stdout = r
            .data
            .get("stdout_b64")
            .and_then(|v| v.as_str())
            .map(|b| {
                base64::engine::general_purpose::STANDARD
                    .decode(b)
                    .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
                    .unwrap_or_default()
            })
            .unwrap_or_default();
        // A holdout run's output tail is not written to the stored log: it is the only free-text channel from the confirm
        // run to whoever reads logs. The parsed numbers still travel in the outcome.
        if !stdout.is_empty() && spec.trace.is_none() {
            self.log.line(&format!("[bench stdout tail]\n{stdout}"));
        }
        let timed_out = r.data.get("timedOut").and_then(serde_json::Value::as_bool) == Some(true);
        let ms = r.data.get("ms").and_then(serde_json::Value::as_f64);
        self.log.line(&format!(
            "[exit {}{} after {}ms]",
            exit_code.map_or_else(|| "null".to_owned(), |c| c.to_string()),
            if timed_out { ", timed out" } else { "" },
            r.data.get("ms").map_or_else(|| "undefined".to_owned(), json_text)
        ));
        Ok(StepOutcome {
            exit_code,
            timed_out,
            stdout,
            duration_ms: ms.filter(|m| *m != 0.0).unwrap_or(0.0),
            tampered: None,
            error: None,
        })
    }

    async fn dispose(&mut self) {
        self.session.terminate().await;
    }
}

/// `String(value)` for a sentinel value.
fn json_text(v: &serde_json::Value) -> String {
    match v {
        serde_json::Value::String(s) => s.clone(),
        other => other.to_string(),
    }
}
