//! The orchestration half of dependency snapshots and container images (port of `remote-deps.ts`). `RemoteBenchRunner` runs
//! it before the first job of a lockfile / Dockerfile, i.e. before the baseline; later jobs only look the manifest up.
//!
//! ```text
//! resolve bench.ref -> strategy + lockfile hashes of the BASE tree (never the candidate) -> key
//!   -> `bench-deps/<key>.json` valid and its archive present?  yes: reuse
//!   -> no: stage the base tree, launch a `deps` instance (agent subnet, egress), run the build script, ALWAYS terminate
//!          the instance in a `finally`, then re-read and re-validate the manifest the job wrote.
//! ```
//!
//! The deps instance carries the same `ratchet:opt` tag as every bench machine, so the reaper and the teardown orphan check
//! (`list_tagged`) cover it exactly like a bench instance that a crash left behind. Concurrent jobs that need the same key
//! share one build.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use ratchet_core::RatchetConfig;
use ratchet_core::ports::{
    ArtifactStore, BenchJob, BenchLaunchPurpose, BenchLaunchSpec, CancellationToken, GitHost,
};
use tokio::sync::{OnceCell, watch};

use crate::deps::{
    DepsKeyInput, DepsManifest, DepsPlan, DepsStrategy, DepsStrategyId, FileHashes,
    arch_from_instance_type, deps_key, hash_named_files, manifest_key, parse_deps_manifest,
    pinned_deps_files, resolve_deps_strategy, root_entries,
};
use crate::deps_scripts::{DepsBuildInput, deps_build_script};
use crate::errors::{BenchError, BenchResult};
use crate::image::{
    ImageKeyInput, ImageManifest, ImagePlan, ResolvedImage, image_key, image_manifest_key,
    image_tag, parse_image_manifest, read_environment, resolve_image_source,
};
use crate::image_scripts::{ContainerOptions, ImageBuildInput, ImageRestore, image_build_script};
use crate::log::BenchLog;
use crate::pipeline::{BenchProgress, ProgressFn};
use crate::remote_scripts::{Isolation, Layout, parse_sentinels};
use crate::remote_session::{InstanceSession, SessionOptions, tail600};
use crate::stage::{StageBase, stage_deps_base};

#[derive(Clone)]
pub struct DepsSnapshotOptions {
    pub session: SessionOptions,
    pub git: Arc<dyn GitHost>,
    /// The evaluator image. It is part of the snapshot key and is also what the dependency job boots.
    pub image_ref: String,
    /// Artifacts bucket and a store that writes to it (the controller's credentials): staging, and reading manifests.
    pub bucket: Option<String>,
    pub store: Option<Arc<dyn ArtifactStore>>,
    pub isolation: Isolation,
    /// Per-job working directories are created under here on the instance.
    pub work_root: String,
    /// Limit for the package-manager fetch.
    pub deps_ms: u64,
    /// Limit for building the repo's container image. Default 45 minutes.
    pub image_ms: Option<u64>,
    /// Container options of the runs (read-only root, pids limit, ...); the tag comes from the image.
    pub container: Option<ContainerOptions>,
    /// Instance type the bench runs on when it is not `evaluator.hardware`: its architecture is part of the key.
    pub bench_instance_type: Option<String>,
    /// Instance type of the snapshot job; must have the architecture of the bench. Default: the bench's.
    pub instance_type: Option<String>,
}

/// What the controller prepares before a job's instance exists: the dependency snapshot and/or the repo's container image.
#[derive(Debug, Clone, Default)]
pub struct PreparedEnvironment {
    pub deps: Option<DepsPlan>,
    pub image: Option<ImagePlan>,
}

pub struct EnsureInput<'a> {
    pub job: &'a BenchJob,
    pub config: &'a RatchetConfig,
    pub log: &'a BenchLog,
    pub cancel: Option<&'a CancellationToken>,
    pub on_progress: Option<ProgressFn<'a>>,
}

struct BaseInfo {
    commit: String,
    strategy: DepsStrategy,
    lockfiles: FileHashes,
    note: Option<String>,
    /// The repo's container (`environment.image`) as resolved at the base commit.
    image: Option<ResolvedImage>,
}

struct BuildWhat {
    arch: String,
    image: Option<(String, ImageKeyInput)>,
    image_have: Option<ImageManifest>,
    image_key: Option<String>,
    deps: Option<String>,
}

type BuildResult = Option<Result<(), BenchError>>;

pub struct DepsSnapshots {
    o: DepsSnapshotOptions,
    bases: Mutex<HashMap<String, Arc<OnceCell<Arc<BaseInfo>>>>>,
    building: Mutex<HashMap<String, watch::Receiver<BuildResult>>>,
}

/// Removes the in-flight build entry when the build ends (or its future is dropped), and tells the waiters how it ended.
struct BuildGuard<'a> {
    map: &'a Mutex<HashMap<String, watch::Receiver<BuildResult>>>,
    key: String,
    tx: watch::Sender<BuildResult>,
    done: bool,
}

impl BuildGuard<'_> {
    fn finish(mut self, result: Result<(), BenchError>) {
        self.done = true;
        self.release(result);
    }

    fn release(&self, result: Result<(), BenchError>) {
        if let Ok(mut m) = self.map.lock() {
            m.remove(&self.key);
        }
        self.tx.send_replace(Some(result));
    }
}

impl Drop for BuildGuard<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.release(Err(BenchError::Aborted));
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl DepsSnapshots {
    pub fn new(o: DepsSnapshotOptions) -> Self {
        DepsSnapshots {
            o,
            bases: Mutex::new(HashMap::new()),
            building: Mutex::new(HashMap::new()),
        }
    }

    /// The plan for this job, or `None` when there is nothing to install from a snapshot (no lockfile, `deps: none`).
    pub async fn ensure(&self, a: &EnsureInput<'_>) -> BenchResult<Option<DepsPlan>> {
        Ok(self.prepare(a, true).await?.deps)
    }

    /// Everything the job needs from an instance WITH egress, prepared once and reused: the dependency snapshot (unless
    /// `want_deps_opt` is false, i.e. the runner has no setup command) and the repo's container image
    /// (`environment.image`). Both are built from bench.ref on ONE throwaway instance when either is missing; with the image
    /// present the snapshot's fetch runs inside it.
    pub async fn prepare(
        &self,
        a: &EnsureInput<'_>,
        want_deps_opt: bool,
    ) -> BenchResult<PreparedEnvironment> {
        let log = a.log;
        let config = a.config;
        let image_path = read_environment(config)?.image;
        let base = self.inspect_base(config, image_path.as_deref()).await?;
        if let Some(n) = &base.note {
            log.line(&format!("dependencies: {n}"));
        }
        let want_deps = want_deps_opt && base.strategy.id != DepsStrategyId::None;
        if !want_deps && want_deps_opt {
            log.line("dependencies: no lockfile to snapshot (or evaluator.deps is none); setup runs with whatever the image provides");
        }
        if !want_deps && base.image.is_none() {
            return Ok(PreparedEnvironment::default());
        }
        let arch = arch_from_instance_type(
            self.o
                .bench_instance_type
                .as_deref()
                .unwrap_or(&config.evaluator.hardware),
        )
        .to_owned();

        // ---- the container image
        let mut img_key: Option<String> = None;
        let mut img_expect: Option<ImageKeyInput> = None;
        let mut have_image: Option<ImageManifest> = None;
        if let Some(bi) = &base.image {
            for n in &bi.spec.notes {
                log.line(&format!("environment: {n}"));
            }
            let ki = ImageKeyInput {
                spec: bi.spec.clone(),
                files: bi.files.clone(),
                arch: arch.clone(),
            };
            let k = image_key(&ki);
            have_image = self.lookup_image(&ki, &k, log).await?;
            if have_image.is_some() {
                log.line(&format!(
                    "environment: reusing container image {k} ({} at {})",
                    bi.spec.path,
                    &base.commit[..base.commit.len().min(12)]
                ));
            }
            img_key = Some(k);
            img_expect = Some(ki);
        }

        // ---- the dependency snapshot (its "image" is the container's key when there is one: the toolchain is part of what
        // it was fetched with)
        let mut d_key: Option<String> = None;
        let mut d_expect: Option<DepsKeyInput> = None;
        let mut have_deps: Option<DepsManifest> = None;
        if want_deps {
            let input = DepsKeyInput {
                strategy: base.strategy.clone(),
                lockfiles: base.lockfiles.clone(),
                image: img_key.clone().unwrap_or_else(|| self.o.image_ref.clone()),
                arch: arch.clone(),
            };
            let k = deps_key(&input);
            have_deps = self.lookup(&k, &input, log).await?;
            if have_deps.is_some() {
                let present: Vec<&str> = base
                    .strategy
                    .lockfiles
                    .iter()
                    .copied()
                    .filter(|f| base.lockfiles.contains_key(*f))
                    .collect();
                log.line(&format!(
                    "dependencies: reusing snapshot {k} ({} at {})",
                    present.join(", "),
                    &base.commit[..base.commit.len().min(12)]
                ));
            }
            d_key = Some(k);
            d_expect = Some(input);
        }

        let need_image = img_key.is_some() && have_image.is_none();
        let need_deps = d_key.is_some() && have_deps.is_none();
        if need_image || need_deps {
            let build_key = format!(
                "{}|{}",
                if need_image {
                    img_key.clone().unwrap_or_default()
                } else {
                    String::new()
                },
                if need_deps {
                    d_key.clone().unwrap_or_default()
                } else {
                    String::new()
                }
            );
            let what = BuildWhat {
                arch: arch.clone(),
                image: if need_image {
                    match (&img_key, &img_expect) {
                        (Some(k), Some(e)) => Some((k.clone(), e.clone())),
                        _ => None,
                    }
                } else {
                    None
                },
                image_have: have_image.clone(),
                image_key: img_key.clone(),
                deps: if need_deps { d_key.clone() } else { None },
            };
            self.build_once(&build_key, a.cancel, || self.build(a, &base, &what))
                .await?;
        }

        let mut image: Option<ImagePlan> = None;
        if let (Some(k), Some(bi), Some(expect)) = (&img_key, &base.image, &img_expect) {
            let made = match have_image.clone() {
                Some(m) => Some(m),
                None => self.lookup_image(expect, k, log).await?,
            };
            let Some(made) = made else {
                return Err(BenchError::infra(format!(
                    "the container image job for {k} finished but its manifest is missing or invalid in the artifacts bucket"
                )));
            };
            image = Some(ImagePlan {
                key: k.clone(),
                tag: image_tag(k),
                spec: bi.spec.clone(),
                files: bi.files.clone(),
                pinned: bi.files.keys().cloned().collect(),
                base_commit: base.commit.clone(),
                manifest: made,
                reused: have_image.is_some(),
            });
        }
        let mut deps: Option<DepsPlan> = None;
        if let (Some(k), Some(expect)) = (&d_key, &d_expect) {
            let made = match have_deps.clone() {
                Some(m) => Some(m),
                None => self.lookup(k, expect, log).await?,
            };
            let Some(made) = made else {
                return Err(BenchError::infra(format!(
                    "the dependency snapshot job for {k} finished but its manifest is missing or invalid in the artifacts bucket"
                )));
            };
            deps = Some(DepsPlan {
                strategy: base.strategy.clone(),
                key: k.clone(),
                base_commit: base.commit.clone(),
                lockfiles: base.lockfiles.clone(),
                manifest: made,
                reused: have_deps.is_some(),
            });
        }
        Ok(PreparedEnvironment { deps, image })
    }

    /// The manifest of the saved image for `key`, if it exists, matches what the controller computed, and its archive is
    /// still in the bucket.
    async fn lookup_image(
        &self,
        expect: &ImageKeyInput,
        key: &str,
        log: &BenchLog,
    ) -> BenchResult<Option<ImageManifest>> {
        let Some(store) = &self.o.store else {
            return Err(BenchError::infra(
                "container images need an artifacts store (stagingStore or artifacts): the bench has no network",
            ));
        };
        let mkey = image_manifest_key(key);
        let text = store
            .get(&mkey)
            .await
            .map_err(|e| BenchError::infra(format!("cannot read {mkey}: {e}")))?;
        let Some(text) = text else { return Ok(None) };
        let manifest = match parse_image_manifest(&text, expect, key) {
            Ok(m) => m,
            Err(error) => {
                log.line(&format!("environment: ignoring {mkey}: {error}"));
                return Ok(None);
            }
        };
        let present = store.list(&manifest.archive).await.unwrap_or_default();
        if !present.contains(&manifest.archive) {
            log.line(&format!(
                "environment: {} is gone from the bucket; rebuilding",
                manifest.archive
            ));
            return Ok(None);
        }
        Ok(Some(manifest))
    }

    /// Strategy, lockfile hashes and (with `environment.image`) the resolved container of bench.ref's tree. Cached per
    /// commit (a commit never changes).
    async fn inspect_base(
        &self,
        config: &RatchetConfig,
        image_path: Option<&str>,
    ) -> BenchResult<Arc<BaseInfo>> {
        let git = self.o.git.clone();
        let mode = config.evaluator.deps;
        let bench_ref = config.evaluator.git_ref.clone();
        let commit = git
            .resolve_ref(&bench_ref)
            .await
            .map_err(|e| BenchError::infra(format!("cannot resolve bench.ref {bench_ref}: {e}")))?;
        let cache_key = format!("{commit}\0{mode}\0{}", image_path.unwrap_or(""));
        let cell = lock(&self.bases)
            .entry(cache_key)
            .or_insert_with(|| Arc::new(OnceCell::new()))
            .clone();
        let image_path = image_path.map(str::to_owned);
        let info = cell
            .get_or_try_init(|| async {
                let dir = tempfile::Builder::new()
                    .prefix("ratchet-deps-base-")
                    .tempdir()
                    .map_err(|e| {
                        BenchError::infra(format!("cannot create a scratch directory: {e}"))
                    })?;
                let tree = dir.path().join("base");
                git.checkout(&commit, &tree).await.map_err(|e| {
                    BenchError::infra(format!("cannot check out bench.ref {bench_ref}: {e}"))
                })?;
                let commit2 = commit.clone();
                let info = tokio::task::spawn_blocking(move || -> BenchResult<BaseInfo> {
                    let resolved = resolve_deps_strategy(mode, root_entries(&tree)?)?;
                    let lockfiles =
                        hash_named_files(&tree, &pinned_deps_files(&resolved.strategy))?;
                    let image = match &image_path {
                        Some(p) => Some(resolve_image_source(&tree, p)?),
                        None => None,
                    };
                    Ok(BaseInfo {
                        commit: commit2,
                        strategy: resolved.strategy,
                        lockfiles,
                        note: resolved.note,
                        image,
                    })
                })
                .await
                .map_err(|e| BenchError::infra(format!("filesystem task failed: {e}")))??;
                drop(dir);
                Ok::<Arc<BaseInfo>, BenchError>(Arc::new(info))
            })
            .await?;
        Ok(info.clone())
    }

    /// The manifest for `key`, if it exists, matches what the controller computed, and its archive is still in the bucket.
    async fn lookup(
        &self,
        key: &str,
        expect: &DepsKeyInput,
        log: &BenchLog,
    ) -> BenchResult<Option<DepsManifest>> {
        let Some(store) = &self.o.store else {
            return Err(BenchError::infra(
                "dependency snapshots need an artifacts store (stagingStore or artifacts): the bench has no network",
            ));
        };
        let mkey = manifest_key(key);
        let text = store
            .get(&mkey)
            .await
            .map_err(|e| BenchError::infra(format!("cannot read {mkey}: {e}")))?;
        let Some(text) = text else { return Ok(None) };
        let manifest = match parse_deps_manifest(&text, expect, key) {
            Ok(m) => m,
            Err(error) => {
                log.line(&format!("dependencies: ignoring {mkey}: {error}"));
                return Ok(None);
            }
        };
        // Lifecycle rules can expire the archive a little before its manifest: do not hand out a manifest without its bytes.
        let present = store.list(&manifest.archive).await.unwrap_or_default();
        if !present.contains(&manifest.archive) {
            log.line(&format!(
                "dependencies: {} is gone from the bucket; rebuilding",
                manifest.archive
            ));
            return Ok(None);
        }
        Ok(Some(manifest))
    }

    async fn build_once<F, Fut>(
        &self,
        key: &str,
        cancel: Option<&CancellationToken>,
        run: F,
    ) -> BenchResult<()>
    where
        F: FnOnce() -> Fut,
        Fut: std::future::Future<Output = BenchResult<()>>,
    {
        loop {
            let existing = lock(&self.building).get(key).cloned();
            let Some(mut rx) = existing else { break };
            let result = loop {
                if let Some(r) = rx.borrow().clone() {
                    break r;
                }
                if rx.changed().await.is_err() {
                    break rx.borrow().clone().unwrap_or(Err(BenchError::Aborted));
                }
            };
            match result {
                Ok(()) => return Ok(()),
                // The job that was building was cancelled: that is not this job's failure, build it ourselves.
                Err(BenchError::Aborted)
                    if !cancel.is_some_and(CancellationToken::is_cancelled) => {}
                Err(e) => return Err(e),
            }
        }
        let (tx, rx) = watch::channel(None);
        lock(&self.building).insert(key.to_owned(), rx);
        let guard = BuildGuard {
            map: &self.building,
            key: key.to_owned(),
            tx,
            done: false,
        };
        let result = run().await;
        guard.finish(result.clone());
        result
    }

    async fn build(&self, a: &EnsureInput<'_>, base: &BaseInfo, w: &BuildWhat) -> BenchResult<()> {
        let (job, config, log, cancel) = (a.job, a.config, a.log, a.cancel);
        let (Some(bucket), Some(store)) = (self.o.bucket.clone(), self.o.store.clone()) else {
            return Err(BenchError::infra(
                "RemoteBenchRunner needs stagingBucket (and an artifacts/stagingStore that writes to it): bench instances have no internet and cannot fetch dependencies themselves",
            ));
        };
        let bench_type = self
            .o
            .bench_instance_type
            .clone()
            .unwrap_or_else(|| config.evaluator.hardware.clone());
        let deps_type = self
            .o
            .instance_type
            .clone()
            .unwrap_or_else(|| bench_type.clone());
        if arch_from_instance_type(&deps_type) != arch_from_instance_type(&bench_type) {
            return Err(BenchError::infra(format!(
                "the dependency snapshot job runs on {deps_type} but the bench on {bench_type}: different architectures would fetch different native packages and images"
            )));
        }
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return Err(BenchError::Aborted);
        }
        if let Some(f) = a.on_progress {
            f(BenchProgress::Deps);
        }
        let what = [
            w.image
                .as_ref()
                .map(|(k, _)| format!("container image {k}")),
            w.deps.as_ref().map(|k| format!("dependency snapshot {k}")),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" + ");
        log.section(&what);
        log.line(&format!(
            "building from bench.ref {} ({}); the candidate is not involved",
            config.evaluator.git_ref, base.commit
        ));

        let staged = stage_deps_base(
            &StageBase {
                git: self.o.git.clone(),
                store: store.clone(),
                bucket: bucket.clone(),
                optimization_id: job.optimization_id.to_string(),
            },
            &base.commit,
        )
        .await?;
        let instance = self
            .o
            .session
            .compute
            .launch_bench_instance(&BenchLaunchSpec {
                optimization_id: job.optimization_id.clone(),
                instance_type: deps_type.clone(),
                image_ref: self.o.image_ref.clone(),
                purpose: BenchLaunchPurpose::Deps,
            })
            .await
            .map_err(|e| {
                BenchError::infra(format!(
                    "cannot launch the dependency snapshot instance: {e}"
                ))
            })?;
        log.line(&format!(
            "dependency snapshot instance {} ({deps_type}) launched",
            instance.id
        ));
        let session = InstanceSession::new(
            self.o.session.clone(),
            instance,
            log.clone(),
            cancel.cloned(),
            "dependency snapshot",
        );
        let result = self
            .build_on(&session, a, base, w, &staged, &bucket, &store)
            .await;
        session.terminate().await;
        result
    }

    #[allow(clippy::too_many_arguments)]
    async fn build_on(
        &self,
        session: &InstanceSession,
        a: &EnsureInput<'_>,
        base: &BaseInfo,
        w: &BuildWhat,
        staged: &crate::remote_scripts::StagedArchive,
        bucket: &str,
        _store: &Arc<dyn ArtifactStore>,
    ) -> BenchResult<()> {
        let (job, log) = (a.job, a.log);
        session.wait_ready().await?;
        let work_dir = |kind: &str| -> String {
            let opt: String = job
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
            let id = uuid::Uuid::new_v4().to_string();
            format!("{}/{kind}-{opt}-{}", self.o.work_root, &id[..8])
        };

        let mut image_manifest = w.image_have.clone();
        if let (Some((key, expect)), Some(bi)) = (&w.image, &base.image) {
            let image_ms = self.o.image_ms.unwrap_or(45 * 60_000);
            let res = session
                .call(
                    "image",
                    &image_build_script(&ImageBuildInput {
                        work: work_dir("image"),
                        base: staged.clone(),
                        key: key.clone(),
                        tag: image_tag(key),
                        spec: bi.spec.clone(),
                        files: bi.files.clone(),
                        arch: w.arch.clone(),
                        bucket: bucket.to_owned(),
                        timeout_ms: image_ms,
                    })?,
                    image_ms + 10 * 60_000,
                )
                .await?;
            let done = parse_sentinels(&res.stdout)
                .into_iter()
                .find(|x| x.kind == "image");
            let Some(done) = done.filter(|_| res.exit_code == 0) else {
                return Err(BenchError::infra(format!(
                    "building the container image from {} failed (exit {}): {}. The image job runs in the agent subnet: the registries the Dockerfile pulls from must be reachable through the egress proxy / DNS allow-list (docs/real-env/aws.md)",
                    bi.spec.path,
                    res.exit_code,
                    tail600(if res.stderr.is_empty() {
                        &res.stdout
                    } else {
                        &res.stderr
                    })
                )));
            };
            log.line(&format!(
                "container image built: {} bytes ({}) in {}ms",
                js(done.data.get("bytes")),
                js(done.data.get("compression")),
                js(done.data.get("ms"))
            ));
            image_manifest = self.lookup_image(expect, key, log).await?;
            if image_manifest.is_none() {
                return Err(BenchError::infra(format!(
                    "the container image job for {key} finished but its manifest is missing or invalid in the artifacts bucket"
                )));
            }
        }

        if let Some(dkey) = &w.deps {
            let container = image_manifest.as_ref().map(|m| ImageRestore {
                uri: format!("s3://{bucket}/{}", m.archive),
                sha256: m.sha256.clone(),
                compression: m.compression,
                image_id: m.image_id.clone(),
                rt: self
                    .o
                    .container
                    .clone()
                    .unwrap_or_default()
                    .runtime(m.tag.clone()),
            });
            let res = session
                .call(
                    "deps",
                    &deps_build_script(&DepsBuildInput {
                        layout: Layout::new(work_dir("deps")),
                        strategy: base.strategy.clone(),
                        base: staged.clone(),
                        key: dkey.clone(),
                        lockfiles: base.lockfiles.clone(),
                        image: w
                            .image_key
                            .clone()
                            .unwrap_or_else(|| self.o.image_ref.clone()),
                        arch: w.arch.clone(),
                        bucket: bucket.to_owned(),
                        timeout_ms: self.o.deps_ms,
                        isolation: self.o.isolation,
                        container,
                    })?,
                    self.o.deps_ms + 5 * 60_000,
                )
                .await?;
            let done = parse_sentinels(&res.stdout)
                .into_iter()
                .find(|x| x.kind == "deps");
            let Some(done) = done.filter(|_| res.exit_code == 0) else {
                return Err(BenchError::infra(format!(
                    "building the {} dependency snapshot failed (exit {}): {}. The snapshot job runs in the agent subnet and needs the package registry through the egress proxy / DNS allow-list (docs/real-env/aws.md)",
                    base.strategy.id,
                    res.exit_code,
                    tail600(if res.stderr.is_empty() {
                        &res.stdout
                    } else {
                        &res.stderr
                    })
                )));
            };
            log.line(&format!(
                "dependency snapshot built: {} bytes ({}) in {}ms",
                js(done.data.get("bytes")),
                js(done.data.get("compression")),
                js(done.data.get("ms"))
            ));
        }
        Ok(())
    }
}

/// `String(value)` for the JSON values the sentinels carry.
fn js(v: Option<&serde_json::Value>) -> String {
    match v {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(other) => other.to_string(),
        None => "undefined".into(),
    }
}
