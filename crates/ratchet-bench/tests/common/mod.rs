//! Shared rigs of the runner tests: a `RemoteBenchRunner` whose bench instance is a local bash.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use ratchet_bench::ProgressBenchRunner;
use ratchet_bench::testkit::{
    ConfigOptions, DirStore, FixtureRepo, InstantClock, LocalFakeCompute, MemoryStore, make_config,
    walk_files,
};
use ratchet_bench::{BenchError, RemoteBenchRunner, RemoteBenchRunnerOptions};
use ratchet_core::RatchetConfig;
use ratchet_core::ports::{BaselineMetrics, BenchJob, BenchJobPhase, BenchOutcome};
use ratchet_core::{CandidateId, OptimizationId};

pub const BUCKET: &str = "ratchet-test-artifacts";

static SEQ: AtomicUsize = AtomicUsize::new(0);

pub fn seq() -> usize {
    SEQ.fetch_add(1, Ordering::SeqCst) + 1
}

pub fn job(sha: &str) -> BenchJob {
    BenchJob {
        optimization_id: OptimizationId::new("opt1"),
        candidate_id: Some(CandidateId::new("cand1")),
        sha: sha.into(),
        phase: BenchJobPhase::Bench,
        repeats: 3,
        trace: None,
    }
}

pub fn job_with(sha: &str, f: impl FnOnce(&mut BenchJob)) -> BenchJob {
    let mut j = job(sha);
    f(&mut j);
    j
}

/// `makeConfig({ guardrails: [], ... })`.
pub fn no_guardrails(f: impl FnOnce(&mut ConfigOptions)) -> RatchetConfig {
    let mut o = ConfigOptions {
        guardrails: Some(vec![]),
        ..ConfigOptions::default()
    };
    f(&mut o);
    make_config(&o)
}

pub fn cfg() -> RatchetConfig {
    no_guardrails(|_| {})
}

pub fn baseline(pairs: &[(&str, f64)]) -> BaselineMetrics {
    BaselineMetrics {
        metrics: pairs.iter().map(|(k, v)| ((*k).to_owned(), *v)).collect(),
    }
}

/// Run a job on any progress runner, with the `BenchError` result (not the port error).
pub async fn run(
    runner: &dyn ProgressBenchRunner,
    j: &BenchJob,
    config: &RatchetConfig,
    baseline: Option<&BaselineMetrics>,
) -> Result<BenchOutcome, BenchError> {
    runner
        .run_with_progress(j, config, baseline, None, None)
        .await
}

pub async fn wait_for(pred: impl Fn() -> bool, ms: u64) {
    let t0 = Instant::now();
    while !pred() {
        assert!(
            t0.elapsed() < Duration::from_millis(ms),
            "wait_for timed out"
        );
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
}

pub struct Rig {
    pub runner: RemoteBenchRunner,
    pub compute: Arc<LocalFakeCompute>,
    pub store: Arc<MemoryStore>,
    pub staging: Arc<DirStore>,
    pub s3: PathBuf,
    pub work_root: PathBuf,
    pub warnings: Arc<Mutex<Vec<String>>>,
    pub clock: Arc<InstantClock>,
    pub flags: PathBuf,
    pub log: PathBuf,
}

impl Rig {
    pub fn flag(&self, name: &str) {
        fs::write(self.flags.join(name), "").expect("flag");
    }

    pub fn unflag(&self, name: &str) {
        let _ = fs::remove_file(self.flags.join(name));
    }

    /// Objects in the artifacts bucket under `prefix`.
    pub fn objects(&self, prefix: &str) -> Vec<String> {
        let mut out = Vec::new();
        walk_files(&self.s3.join(BUCKET), "", &mut out);
        out.retain(|k| k.starts_with(prefix));
        out.sort();
        out
    }

    pub fn warnings(&self) -> Vec<String> {
        self.warnings.lock().map(|w| w.clone()).unwrap_or_default()
    }

    /// The shim log (`echo ... >> $LOG` lines).
    pub fn shim_log(&self) -> String {
        fs::read_to_string(&self.log).unwrap_or_default()
    }

    pub fn bucket_path(&self, key: &str) -> PathBuf {
        self.s3.join(BUCKET).join(key)
    }

    /// `launch:<purpose>` / `terminate:<id>` in the order they happened.
    pub fn timeline(&self) -> Vec<String> {
        self.compute.timeline()
    }
}

/// A `RemoteBenchRunner` over `LocalFakeCompute` (scripts run by local bash, `aws s3 cp` is a directory copy). `tweak` edits
/// the runner options before it is built (`isolation: user` also points the candidate launcher at the shim of that name).
pub fn rig(
    fx: &FixtureRepo,
    extra_env: &[(&str, &str)],
    tweak: impl FnOnce(&mut RemoteBenchRunnerOptions),
) -> Rig {
    let n = seq();
    let s3 = fx.path().join(format!("s3-{n}"));
    let flags = fx.path().join(format!("flags-{n}"));
    fs::create_dir_all(&flags).expect("flags");
    let compute = Arc::new(LocalFakeCompute::new(
        fx.path().join(format!("ssm-{n}")),
        extra_env,
        Some(&s3),
    ));
    let store = MemoryStore::new();
    let staging = DirStore::new(s3.join(BUCKET));
    let clock = InstantClock::new();
    let warnings: Arc<Mutex<Vec<String>>> = Arc::default();
    let work_root = fx.path().join(format!("work-{n}"));
    let mut o =
        RemoteBenchRunnerOptions::new(compute.clone(), fx.host.clone(), clock.clone(), "ami-test");
    o.artifacts = Some(store.clone());
    o.staging_bucket = Some(BUCKET.into());
    o.staging_store = Some(staging.clone());
    o.isolation = ratchet_bench::Isolation::None; // scripts run as the developer here; the 'user' path is exercised with shims
    o.work_root = Some(work_root.to_string_lossy().into_owned());
    let sink = warnings.clone();
    o.on_warning = Some(Arc::new(move |w: &str| {
        sink.lock().expect("lock").push(w.to_owned())
    }));
    tweak(&mut o);
    if o.isolation == ratchet_bench::Isolation::User && o.candidate_launcher.is_none() {
        o.candidate_launcher = Some(compute.shim_path("ratchet-candidate"));
    }
    let log = fx.path().join(format!("shim-{n}.log"));
    Rig {
        runner: RemoteBenchRunner::new(o),
        compute,
        store,
        staging,
        s3,
        work_root,
        warnings,
        clock,
        flags,
        log,
    }
}

/// The shims that stand in for the OS tools of an `isolation: user` instance (the real users, iptables and sudo are image
/// work, see docs/real-env/bench.md). `overrides` replace the defaults by name.
pub fn user_shims(log: &Path, overrides: &[(&str, &str)]) -> Vec<(String, String)> {
    let l = log.display();
    let mut shims: Vec<(String, String)> = vec![
        ("id".into(), r#"[ "$2" = ratchet-bench ] || [ "$2" = ratchet-holdout ] && exit 0; exec /usr/bin/id "$@""#.into()),
        ("iptables".into(), format!(r#"echo "iptables $*" >> {l}; exit 0"#)),
        (
            "setpriv".into(),
            format!(
                r#"echo "setpriv $*" >> {l}; if [[ "$*" == *"/dev/tcp/169.254.169.254"* ]]; then exit 1; fi; if [[ "$*" == *"--reuid=ratchet-bench"* && "$*" == *"holdout/.probe"* ]]; then exit 1; fi; while [ "$1" != -- ]; do shift; done; shift; exec "$@""#
            ),
        ),
        ("pkill".into(), format!(r#"echo "pkill $*" >> {l}; exit 0"#)),
        ("chown".into(), format!(r#"echo "chown $*" >> {l}; exit 0"#)),
        (
            "ratchet-candidate".into(),
            format!(r#"echo "launcher $*" >> {l}; if [ "$*" = "id -un" ]; then echo ratchet-bench; exit 0; fi; exec env "$@""#),
        ),
    ];
    for (name, body) in overrides {
        shims.retain(|(n, _)| n != name);
        shims.push(((*name).to_owned(), (*body).to_owned()));
    }
    shims
}

pub fn install(compute: &LocalFakeCompute, shims: &[(String, String)]) {
    for (n, b) in shims {
        compute.add_shim(n, b);
    }
}

/// A fixture-repo-backed rig with the user-isolation shims installed (the shim log is `rig.log`).
pub fn user_rig(
    fx: &FixtureRepo,
    overrides: &[(&str, &str)],
    tweak: impl FnOnce(&mut RemoteBenchRunnerOptions),
) -> Rig {
    let r = rig(fx, &[], |o| {
        o.isolation = ratchet_bench::Isolation::User;
        tweak(o);
    });
    install(&r.compute, &user_shims(&r.log, overrides));
    r
}

pub fn list_children(dir: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = fs::read_dir(dir)
        .map(|rd| rd.filter_map(Result::ok).map(|e| e.path()).collect())
        .unwrap_or_default();
    v.sort();
    v
}
