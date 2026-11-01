//! In-memory fakes of every port, scripted for engine tests (feature `testing`). Port of
//! `packages/engine/src/testing/{fakes,harness}.ts`.
//!
//! All fakes append to one shared [`CallLog`] so tests can assert cross-port ordering (kill switch). Hooks (`*_hook`)
//! replace the TypeScript tests' monkey patching of fake methods.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::future::Future;
use std::path::{Path, PathBuf};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use async_trait::async_trait;
use indexmap::IndexMap;
use ratchet_core::config::{GuardrailResult, RatchetConfig};
use ratchet_core::enums::{AgentKind, AgentsWhere, InstanceRole, Mode};
use ratchet_core::ids::{AgentId, OptimizationId};
use ratchet_core::ports::mock::ManualClock;
use ratchet_core::ports::{
    AgentEvent, AgentIdentity, ArtifactStore, BaselineMetrics, BenchJob, BenchLaunchSpec,
    BenchOutcome, BenchRun, BenchRunner, CancellationToken, CandidateCommit, Clock, CloudCompute,
    CommitFilesInput, CommitRef, CostReconciler, EngineEvent, FileDiff, GitHost, InstanceInfo,
    InstanceState, PlanAction, PlanChange, PolledEvents, PortError, PortResult, PriceBook,
    PullRequestInput, PullRequestRef, RemoteCommand, RemoteCommandResult, RunnerConfig,
    RunnerControl, RunnerHandle, RunnerLaunchSpec, RunnerMessage, SandboxInput, SandboxOutputs,
    SandboxPlan, SandboxProvider, SecretRef,
};
use ratchet_ledger::{Ledger, LedgerOptions};
use tokio::sync::{broadcast, watch};

use crate::engine::{CreateOptimizationInput, Engine, EngineOptions};
use crate::ports::{EnginePorts, GitSource};
use crate::settings::EngineSettings;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

pub type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// One shared, ordered log of port calls.
#[derive(Clone, Default)]
pub struct CallLog(Arc<Mutex<Vec<String>>>);

impl CallLog {
    pub fn push(&self, s: impl Into<String>) {
        lock(&self.0).push(s.into());
    }

    pub fn all(&self) -> Vec<String> {
        lock(&self.0).clone()
    }

    pub fn len(&self) -> usize {
        lock(&self.0).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn since(&self, mark: usize) -> Vec<String> {
        lock(&self.0).iter().skip(mark).cloned().collect()
    }

    pub fn starting_with(&self, prefix: &str) -> Vec<String> {
        lock(&self.0)
            .iter()
            .filter(|l| l.starts_with(prefix))
            .cloned()
            .collect()
    }
}

pub const T0: i64 = 1_700_000_000_000;

pub fn fake_clock() -> Arc<ManualClock> {
    Arc::new(ManualClock::new(T0))
}

// ---------------------------------------------------------------- git

#[derive(Clone)]
pub struct FakeCommit {
    pub commit: CandidateCommit,
    pub files: Vec<String>,
}

pub type ListHook = Arc<
    dyn Fn(String, Option<String>) -> BoxFut<Option<PortResult<Vec<CandidateCommit>>>>
        + Send
        + Sync,
>;

pub struct FakeGit {
    log: CallLog,
    clock: Arc<dyn Clock>,
    pub branches: Mutex<BTreeMap<String, String>>,
    pub commits: Mutex<HashMap<String, Vec<FakeCommit>>>,
    pub prs: Mutex<Vec<PullRequestInput>>,
    pub fail_changed_files: Mutex<HashSet<String>>,
    pub files: Mutex<BTreeMap<String, String>>,
    /// Branches that already exist in the remote (`commit_files` refuses them).
    pub existing_branches: Mutex<HashSet<String>>,
    pub files_committed: Mutex<Vec<(CommitFilesInput, String)>>,
    pub fail_commit_files: AtomicBool,
    pub resolved: Mutex<HashMap<String, String>>,
    /// Ignore `since_sha` when listing (a host that re-lists everything).
    pub ignore_since: AtomicBool,
    /// Replaces `list_candidates` when it returns `Some`.
    pub list_hook: Mutex<Option<ListHook>>,
    /// Does `commit_files` exist at all (a host that cannot commit files answers `Unsupported`).
    pub can_commit_files: AtomicBool,
}

impl FakeGit {
    pub fn new(log: CallLog, clock: Arc<dyn Clock>, files: &[(&str, &str)]) -> Self {
        Self {
            log,
            clock,
            branches: Mutex::new(BTreeMap::new()),
            commits: Mutex::new(HashMap::new()),
            prs: Mutex::new(Vec::new()),
            fail_changed_files: Mutex::new(HashSet::new()),
            files: Mutex::new(
                files
                    .iter()
                    .map(|(a, b)| ((*a).to_owned(), (*b).to_owned()))
                    .collect(),
            ),
            existing_branches: Mutex::new(HashSet::new()),
            files_committed: Mutex::new(Vec::new()),
            fail_commit_files: AtomicBool::new(false),
            resolved: Mutex::new(HashMap::new()),
            ignore_since: AtomicBool::new(false),
            list_hook: Mutex::new(None),
            can_commit_files: AtomicBool::new(true),
        }
    }

    /// Script a candidate commit on `branch` (what the runner would push).
    pub fn push(&self, branch: &str, sha: &str, changed: &[&str]) {
        self.push_with_message(
            branch,
            sha,
            changed,
            &format!("perf: {sha}\n\nRatchet-Candidate: true"),
        );
    }

    pub fn push_with_message(&self, branch: &str, sha: &str, changed: &[&str], message: &str) {
        lock(&self.commits)
            .entry(branch.to_owned())
            .or_default()
            .push(FakeCommit {
                commit: CandidateCommit {
                    sha: sha.to_owned(),
                    branch: branch.to_owned(),
                    message: message.to_owned(),
                    committed_at: self.clock.now(),
                },
                files: changed.iter().map(|s| (*s).to_owned()).collect(),
            });
    }

    pub fn set_file(&self, path: &str, body: &str) {
        lock(&self.files).insert(path.to_owned(), body.to_owned());
    }

    pub fn remove_file(&self, path: &str) {
        lock(&self.files).remove(path);
    }

    pub fn pr_count(&self) -> usize {
        lock(&self.prs).len()
    }

    pub fn pr(&self, i: usize) -> PullRequestInput {
        lock(&self.prs)[i].clone()
    }
}

#[async_trait]
impl GitHost for FakeGit {
    async fn create_branch(&self, from_ref: &str, branch: &str) -> PortResult<()> {
        self.log.push(format!("git.createBranch:{branch}"));
        lock(&self.branches).insert(branch.to_owned(), from_ref.to_owned());
        Ok(())
    }

    async fn list_candidates(
        &self,
        branch: &str,
        since_sha: Option<&str>,
    ) -> PortResult<Vec<CandidateCommit>> {
        let hook = lock(&self.list_hook).clone();
        if let Some(h) = hook
            && let Some(r) = h(branch.to_owned(), since_sha.map(str::to_owned)).await
        {
            return r;
        }
        let list = lock(&self.commits).get(branch).cloned().unwrap_or_default();
        let idx = if self.ignore_since.load(Ordering::SeqCst) {
            None
        } else {
            since_sha.and_then(|s| list.iter().position(|c| c.commit.sha == s))
        };
        Ok(list
            .into_iter()
            .skip(idx.map_or(0, |i| i + 1))
            .map(|c| c.commit)
            .collect())
    }

    async fn changed_files(&self, _base_ref: &str, sha: &str) -> PortResult<Vec<String>> {
        if lock(&self.fail_changed_files).contains(sha) {
            return Err(PortError::failed(format!("cannot diff {sha}")));
        }
        for list in lock(&self.commits).values() {
            if let Some(c) = list.iter().find(|x| x.commit.sha == sha) {
                return Ok(c.files.clone());
            }
        }
        Err(PortError::failed(format!("unknown sha {sha}")))
    }

    async fn diff(&self, base_ref: &str, sha: &str) -> PortResult<Vec<FileDiff>> {
        Ok(self
            .changed_files(base_ref, sha)
            .await?
            .into_iter()
            .map(|path| FileDiff {
                path,
                additions: 1,
                deletions: 0,
                binary: false,
                patch: "@@ -0,0 +1 @@\n+changed".into(),
            })
            .collect())
    }

    async fn checkout(&self, _sha: &str, dir: &Path) -> PortResult<()> {
        let files = lock(&self.files).clone();
        for (path, body) in files {
            let full = dir.join(path);
            if let Some(p) = full.parent() {
                tokio::fs::create_dir_all(p).await?;
            }
            tokio::fs::write(full, body).await?;
        }
        Ok(())
    }

    async fn open_pull_request(&self, input: &PullRequestInput) -> PortResult<PullRequestRef> {
        self.log.push("git.openPullRequest");
        let mut prs = lock(&self.prs);
        prs.push(input.clone());
        Ok(PullRequestRef {
            url: format!("https://example.test/pr/{}", prs.len()),
        })
    }

    async fn resolve_ref(&self, git_ref: &str) -> PortResult<String> {
        Ok(lock(&self.resolved)
            .get(git_ref)
            .cloned()
            .unwrap_or_else(|| format!("{git_ref}-sha")))
    }

    fn remote_url(&self) -> String {
        "https://example.test/acme/repo.git".into()
    }

    async fn commit_files(&self, input: &CommitFilesInput) -> PortResult<CommitRef> {
        if !self.can_commit_files.load(Ordering::SeqCst) {
            return Err(PortError::Unsupported("commit_files"));
        }
        self.log.push(format!("git.commitFiles:{}", input.branch));
        if self.fail_commit_files.load(Ordering::SeqCst)
            || !lock(&self.existing_branches).insert(input.branch.clone())
        {
            return Err(PortError::failed(format!(
                "branch {} already exists",
                input.branch
            )));
        }
        let mut done = lock(&self.files_committed);
        let sha = format!("files-{}", done.len() + 1);
        done.push((input.clone(), sha.clone()));
        Ok(CommitRef { sha })
    }
}

// ---------------------------------------------------------------- bench

#[derive(Clone, Default)]
pub struct ScriptedBench {
    pub samples: Vec<f64>,
    pub guardrails: Vec<GuardrailResult>,
    pub metrics: BTreeMap<String, f64>,
    /// Fail this many times before succeeding.
    pub fail_times: u32,
}

impl ScriptedBench {
    pub fn samples(samples: &[f64]) -> Self {
        Self {
            samples: samples.to_vec(),
            ..Default::default()
        }
    }

    pub fn with_guardrails(mut self, g: Vec<GuardrailResult>) -> Self {
        self.guardrails = g;
        self
    }

    pub fn with_metric(mut self, k: &str, v: f64) -> Self {
        self.metrics.insert(k.to_owned(), v);
        self
    }

    pub fn failing(mut self, times: u32) -> Self {
        self.fail_times = times;
        self
    }
}

pub fn passed(name: &str) -> GuardrailResult {
    GuardrailResult {
        name: name.to_owned(),
        passed: true,
        detail: None,
    }
}

pub fn failed(name: &str, detail: Option<&str>) -> GuardrailResult {
    GuardrailResult {
        name: name.to_owned(),
        passed: false,
        detail: detail.map(str::to_owned),
    }
}

pub struct FakeBench {
    log: CallLog,
    clock: Arc<dyn Clock>,
    pub jobs: Mutex<Vec<BenchJob>>,
    script: Mutex<HashMap<String, ScriptedBench>>,
    gate: watch::Sender<bool>,
    pub baseline_args: Mutex<Vec<Option<BaselineMetrics>>>,
    failed: Mutex<HashMap<String, u32>>,
}

impl FakeBench {
    pub fn new(log: CallLog, clock: Arc<dyn Clock>) -> Self {
        Self {
            log,
            clock,
            jobs: Mutex::new(Vec::new()),
            script: Mutex::new(HashMap::new()),
            gate: watch::channel(false).0,
            baseline_args: Mutex::new(Vec::new()),
            failed: Mutex::new(HashMap::new()),
        }
    }

    pub fn set(&self, sha: &str, phase: &str, s: ScriptedBench) {
        lock(&self.script).insert(format!("{sha}:{phase}"), s);
    }

    /// Hold every bench run until [`open_gate`](Self::open_gate) (or its cancellation).
    pub fn close_gate(&self) {
        self.gate.send_replace(true);
    }

    pub fn open_gate(&self) {
        self.gate.send_replace(false);
    }

    pub fn jobs(&self) -> Vec<BenchJob> {
        lock(&self.jobs).clone()
    }

    /// `sha:phase` of every job.
    pub fn job_names(&self) -> Vec<String> {
        lock(&self.jobs)
            .iter()
            .map(|j| format!("{}:{}", j.sha, phase_str(j.phase)))
            .collect()
    }

    pub fn job_count(&self) -> usize {
        lock(&self.jobs).len()
    }
}

pub fn phase_str(p: ratchet_core::ports::BenchJobPhase) -> &'static str {
    match p {
        ratchet_core::ports::BenchJobPhase::Baseline => "baseline",
        ratchet_core::ports::BenchJobPhase::Bench => "bench",
        ratchet_core::ports::BenchJobPhase::Confirm => "confirm",
    }
}

#[async_trait]
impl BenchRunner for FakeBench {
    async fn run(
        &self,
        job: &BenchJob,
        _config: &RatchetConfig,
        baseline: Option<&BaselineMetrics>,
        cancel: Option<&CancellationToken>,
    ) -> PortResult<BenchOutcome> {
        self.log
            .push(format!("bench.{}:{}", phase_str(job.phase), job.sha));
        lock(&self.jobs).push(job.clone());
        lock(&self.baseline_args).push(baseline.cloned());
        let aborted = || PortError::failed("aborted");
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return Err(aborted());
        }
        let mut rx = self.gate.subscribe();
        while *rx.borrow() {
            tokio::select! {
                r = rx.changed() => { if r.is_err() { break; } }
                () = async { match cancel { Some(c) => c.cancelled().await, None => std::future::pending().await } } => {
                    return Err(aborted());
                }
            }
        }
        if cancel.is_some_and(CancellationToken::is_cancelled) {
            return Err(aborted());
        }
        let key = format!("{}:{}", job.sha, phase_str(job.phase));
        let Some(s) = lock(&self.script).get(&key).cloned() else {
            return Err(PortError::failed(format!(
                "no scripted bench result for {key}"
            )));
        };
        if s.fail_times > 0 {
            let mut f = lock(&self.failed);
            let n = f.entry(key.clone()).or_default();
            if *n < s.fail_times {
                *n += 1;
                return Err(PortError::failed(format!(
                    "bench infrastructure hiccup #{n}"
                )));
            }
        }
        let started_at = self.clock.now();
        let runs = s
            .samples
            .iter()
            .map(|v| {
                let mut metrics = s.metrics.clone();
                metrics.insert("objective".into(), *v);
                BenchRun {
                    value: *v,
                    metrics,
                    duration_ms: 1000,
                }
            })
            .collect();
        Ok(BenchOutcome {
            runs,
            samples: s.samples.clone(),
            guardrails: s.guardrails.clone(),
            logs_key: None,
            started_at,
            finished_at: started_at + 1000,
        })
    }
}

// ---------------------------------------------------------------- runners

pub type StopHook = Arc<dyn Fn(&RunnerHandle) + Send + Sync>;

pub struct FakeRunners {
    log: CallLog,
    pub started: Mutex<Vec<(RunnerHandle, RunnerConfig)>>,
    pub sent: Mutex<Vec<(String, RunnerMessage)>>,
    pub stopped: Mutex<Vec<String>>,
    pub released: Mutex<Vec<String>>,
    pub poll_calls: Mutex<Vec<(String, Option<String>)>>,
    queues: Mutex<HashMap<String, Vec<AgentEvent>>>,
    pub fail_poll: Mutex<HashSet<String>>,
    pub fail_send: Mutex<HashSet<String>>,
    /// Called after a stop is recorded.
    pub stop_hook: Mutex<Option<StopHook>>,
}

impl FakeRunners {
    pub fn new(log: CallLog) -> Self {
        Self {
            log,
            started: Mutex::new(Vec::new()),
            sent: Mutex::new(Vec::new()),
            stopped: Mutex::new(Vec::new()),
            released: Mutex::new(Vec::new()),
            poll_calls: Mutex::new(Vec::new()),
            queues: Mutex::new(HashMap::new()),
            fail_poll: Mutex::new(HashSet::new()),
            fail_send: Mutex::new(HashSet::new()),
            stop_hook: Mutex::new(None),
        }
    }

    /// Script events an agent's runner will report (appended after existing ones).
    pub fn emit(&self, agent_id: &str, events: Vec<AgentEvent>) {
        lock(&self.queues)
            .entry(agent_id.to_owned())
            .or_default()
            .extend(events);
    }

    pub fn sent_to(&self, agent_id: &str) -> Vec<RunnerMessage> {
        lock(&self.sent)
            .iter()
            .filter(|(a, _)| a == agent_id)
            .map(|(_, m)| m.clone())
            .collect()
    }
}

#[async_trait]
impl RunnerControl for FakeRunners {
    async fn start(&self, handle: &RunnerHandle, config: &RunnerConfig) -> PortResult<()> {
        self.log.push(format!("runners.start:{}", handle.agent_id));
        lock(&self.started).push((handle.clone(), config.clone()));
        Ok(())
    }

    async fn send(&self, handle: &RunnerHandle, message: &RunnerMessage) -> PortResult<()> {
        self.log.push(format!(
            "runners.send:{}:{}",
            handle.agent_id,
            serde_json::to_value(message)
                .ok()
                .and_then(|v| v.get("type").and_then(|t| t.as_str()).map(str::to_owned))
                .unwrap_or_default()
        ));
        if lock(&self.fail_send).contains(handle.agent_id.as_str()) {
            return Err(PortError::failed("ssm send failed"));
        }
        lock(&self.sent).push((handle.agent_id.to_string(), message.clone()));
        Ok(())
    }

    /// Cursor is the index into the agent's event list, as a string.
    async fn poll(&self, handle: &RunnerHandle, cursor: Option<&str>) -> PortResult<PolledEvents> {
        lock(&self.poll_calls).push((handle.agent_id.to_string(), cursor.map(str::to_owned)));
        if lock(&self.fail_poll).contains(handle.agent_id.as_str()) {
            return Err(PortError::failed("s3 unavailable"));
        }
        let all = lock(&self.queues)
            .get(handle.agent_id.as_str())
            .cloned()
            .unwrap_or_default();
        let from = cursor.and_then(|c| c.parse::<usize>().ok()).unwrap_or(0);
        Ok(PolledEvents {
            events: all.iter().skip(from).cloned().collect(),
            cursor: all.len().to_string(),
        })
    }

    async fn stop(&self, handle: &RunnerHandle) -> PortResult<()> {
        self.log.push(format!("runners.stop:{}", handle.agent_id));
        lock(&self.stopped).push(handle.agent_id.to_string());
        let hook = lock(&self.stop_hook).clone();
        if let Some(h) = hook {
            h(handle);
        }
        Ok(())
    }

    async fn release(&self, optimization_id: &OptimizationId) -> PortResult<()> {
        self.log.push(format!("runners.release:{optimization_id}"));
        lock(&self.released).push(optimization_id.to_string());
        Ok(())
    }
}

// ---------------------------------------------------------------- compute

#[derive(Clone)]
pub struct FakeIdentity {
    pub identity: AgentIdentity,
    pub revoked: bool,
    pub deleted: bool,
}

pub struct FakeCompute {
    log: CallLog,
    clock: Arc<dyn Clock>,
    pub instances: Mutex<IndexMap<String, InstanceInfo>>,
    pub identities: Mutex<IndexMap<String, FakeIdentity>>,
    pub secrets: Mutex<BTreeMap<String, String>>,
    pub deleted_secrets_for: Mutex<HashSet<String>>,
    /// Instances with these ids ignore `terminate` (to prove orphan reporting).
    pub stubborn: Mutex<HashSet<String>>,
    pub fail_revoke: Mutex<HashSet<String>>,
    pub fail_delete_secrets: AtomicBool,
    /// Make `list_tagged` fail (the orphan check cannot run).
    pub fail_list_tagged: Mutex<Option<PortError>>,
    /// The n-th `launch_runner` call (1-based) never returns (a process that died mid-launch).
    pub hang_launch_at: AtomicUsize,
    launches: AtomicUsize,
    n: AtomicUsize,
}

impl FakeCompute {
    pub fn new(log: CallLog, clock: Arc<dyn Clock>) -> Self {
        Self {
            log,
            clock,
            instances: Mutex::new(IndexMap::new()),
            identities: Mutex::new(IndexMap::new()),
            secrets: Mutex::new(BTreeMap::new()),
            deleted_secrets_for: Mutex::new(HashSet::new()),
            stubborn: Mutex::new(HashSet::new()),
            fail_revoke: Mutex::new(HashSet::new()),
            fail_delete_secrets: AtomicBool::new(false),
            fail_list_tagged: Mutex::new(None),
            hang_launch_at: AtomicUsize::new(0),
            launches: AtomicUsize::new(0),
            n: AtomicUsize::new(0),
        }
    }

    fn add(
        &self,
        role: InstanceRole,
        optimization_id: &OptimizationId,
        agent_id: Option<AgentId>,
        instance_type: &str,
    ) -> InstanceInfo {
        let inst = InstanceInfo {
            id: format!("i-{:04}", self.n.fetch_add(1, Ordering::SeqCst) + 1),
            role,
            optimization_id: optimization_id.clone(),
            agent_id,
            instance_type: instance_type.to_owned(),
            state: InstanceState::Running,
            launched_at: self.clock.now(),
            terminated_at: None,
        };
        lock(&self.instances).insert(inst.id.clone(), inst.clone());
        inst
    }

    /// Test helper: an agent's experiment instance.
    pub fn launch_experiment(&self, optimization_id: &str, agent_id: &str) -> InstanceInfo {
        self.add(
            InstanceRole::Experiment,
            &OptimizationId::new(optimization_id),
            Some(AgentId::new(agent_id)),
            "c7i.xlarge",
        )
    }

    pub fn live(&self, optimization_id: Option<&str>) -> Vec<InstanceInfo> {
        lock(&self.instances)
            .values()
            .filter(|i| {
                i.state != InstanceState::Terminated
                    && optimization_id.is_none_or(|o| i.optimization_id.as_str() == o)
            })
            .cloned()
            .collect()
    }

    pub fn instance(&self, id: &str) -> Option<InstanceInfo> {
        lock(&self.instances).get(id).cloned()
    }

    pub fn identity(&self, agent_id: &str) -> Option<FakeIdentity> {
        lock(&self.identities).get(agent_id).cloned()
    }

    pub fn secret_count(&self) -> usize {
        lock(&self.secrets).len()
    }
}

#[async_trait]
impl CloudCompute for FakeCompute {
    async fn create_agent_identity(
        &self,
        optimization_id: &OptimizationId,
        agent_id: &AgentId,
    ) -> PortResult<AgentIdentity> {
        self.log
            .push(format!("compute.createAgentIdentity:{agent_id}"));
        let id = AgentIdentity {
            optimization_id: optimization_id.clone(),
            agent_id: agent_id.clone(),
            role_ref: format!("role/{agent_id}"),
            instance_profile_ref: format!("profile/{agent_id}"),
        };
        lock(&self.identities).insert(
            agent_id.to_string(),
            FakeIdentity {
                identity: id.clone(),
                revoked: false,
                deleted: false,
            },
        );
        Ok(id)
    }

    async fn revoke_agent_identity(&self, identity: &AgentIdentity) -> PortResult<()> {
        self.log
            .push(format!("compute.revoke:{}", identity.agent_id));
        if lock(&self.fail_revoke).contains(identity.agent_id.as_str()) {
            return Err(PortError::failed("iam throttled"));
        }
        if let Some(cur) = lock(&self.identities).get_mut(identity.agent_id.as_str()) {
            cur.revoked = true;
        }
        Ok(())
    }

    async fn delete_agent_identity(&self, identity: &AgentIdentity) -> PortResult<()> {
        self.log
            .push(format!("compute.deleteIdentity:{}", identity.agent_id));
        if let Some(cur) = lock(&self.identities).get_mut(identity.agent_id.as_str()) {
            cur.deleted = true;
        }
        Ok(())
    }

    async fn put_secret(
        &self,
        optimization_id: &OptimizationId,
        agent_id: &AgentId,
        name: &str,
        value: &str,
    ) -> PortResult<SecretRef> {
        self.log
            .push(format!("compute.putSecret:{agent_id}:{name}"));
        lock(&self.secrets).insert(
            format!("{optimization_id}/{agent_id}/{name}"),
            value.to_owned(),
        );
        Ok(SecretRef {
            name: name.to_owned(),
            reference: format!("/ratchet/{optimization_id}/{agent_id}/{name}"),
        })
    }

    async fn delete_secrets(
        &self,
        optimization_id: &OptimizationId,
        agent_id: &AgentId,
    ) -> PortResult<()> {
        self.log.push(format!("compute.deleteSecrets:{agent_id}"));
        if self.fail_delete_secrets.load(Ordering::SeqCst) {
            return Err(PortError::failed("ssm denied"));
        }
        let prefix = format!("{optimization_id}/{agent_id}/");
        lock(&self.secrets).retain(|k, _| !k.starts_with(&prefix));
        lock(&self.deleted_secrets_for).insert(agent_id.to_string());
        Ok(())
    }

    async fn launch_runner(&self, spec: &RunnerLaunchSpec) -> PortResult<InstanceInfo> {
        let n = self.launches.fetch_add(1, Ordering::SeqCst) + 1;
        self.log
            .push(format!("compute.launchRunner:{}", spec.agent_id));
        if self.hang_launch_at.load(Ordering::SeqCst) == n {
            std::future::pending::<()>().await;
        }
        Ok(self.add(
            InstanceRole::AgentRunner,
            &spec.optimization_id,
            Some(spec.agent_id.clone()),
            &spec.instance_type,
        ))
    }

    async fn launch_bench_instance(&self, spec: &BenchLaunchSpec) -> PortResult<InstanceInfo> {
        Ok(self.add(
            InstanceRole::Bench,
            &spec.optimization_id,
            None,
            &spec.instance_type,
        ))
    }

    async fn terminate(&self, instance_ids: &[String]) -> PortResult<()> {
        self.log
            .push(format!("compute.terminate:{}", instance_ids.join(",")));
        let stubborn = lock(&self.stubborn).clone();
        let mut instances = lock(&self.instances);
        for id in instance_ids {
            if stubborn.contains(id) {
                continue;
            }
            if let Some(i) = instances.get_mut(id) {
                i.state = InstanceState::Terminated;
                i.terminated_at = Some(self.clock.now());
            }
        }
        Ok(())
    }

    async fn list_tagged(
        &self,
        optimization_id: Option<&OptimizationId>,
    ) -> PortResult<Vec<InstanceInfo>> {
        if let Some(e) = lock(&self.fail_list_tagged).clone() {
            return Err(e);
        }
        Ok(lock(&self.instances)
            .values()
            .filter(|i| optimization_id.is_none_or(|o| &i.optimization_id == o))
            .cloned()
            .collect())
    }

    async fn run_command(&self, _id: &str, _c: &RemoteCommand) -> PortResult<RemoteCommandResult> {
        Ok(RemoteCommandResult {
            exit_code: 0,
            stdout: String::new(),
            stderr: String::new(),
        })
    }
}

pub struct FakePrices {
    prices: BTreeMap<String, f64>,
    fallback: f64,
}

impl FakePrices {
    pub fn new(fallback: f64) -> Self {
        Self {
            prices: BTreeMap::new(),
            fallback,
        }
    }
}

#[async_trait]
impl PriceBook for FakePrices {
    async fn hourly_usd(&self, instance_type: &str) -> PortResult<f64> {
        Ok(self
            .prices
            .get(instance_type)
            .copied()
            .unwrap_or(self.fallback))
    }
}

#[derive(Default)]
pub struct FakeArtifacts {
    pub store: Mutex<BTreeMap<String, Vec<u8>>>,
}

impl FakeArtifacts {
    pub fn text(&self, key: &str) -> Option<String> {
        lock(&self.store)
            .get(key)
            .map(|b| String::from_utf8_lossy(b).into_owned())
    }
}

#[async_trait]
impl ArtifactStore for FakeArtifacts {
    async fn put(&self, key: &str, body: &[u8]) -> PortResult<()> {
        lock(&self.store).insert(key.to_owned(), body.to_vec());
        Ok(())
    }

    async fn get(&self, key: &str) -> PortResult<Option<String>> {
        Ok(self.text(key))
    }

    async fn list(&self, prefix: &str) -> PortResult<Vec<String>> {
        Ok(lock(&self.store)
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect())
    }
}

pub struct FakeSandbox {
    log: CallLog,
    pub applied: Mutex<BTreeMap<String, SandboxOutputs>>,
    /// Make the next apply / destroy fail.
    pub fail_apply: Mutex<Option<PortError>>,
    pub fail_destroy: Mutex<Option<PortError>>,
    /// Called at the start of apply (tests observe state there).
    pub on_apply: Mutex<Option<Arc<dyn Fn(&SandboxInput) + Send + Sync>>>,
    pub applies: Mutex<Vec<SandboxInput>>,
    /// When set, the ledger status of the sandbox is recorded at the start of every apply / destroy.
    pub observe: Mutex<Option<Ledger>>,
    pub status_during_apply: Mutex<Vec<String>>,
    pub status_during_destroy: Mutex<Vec<String>>,
    pub destroy_inputs: Mutex<Vec<SandboxInput>>,
}

impl FakeSandbox {
    pub fn new(log: CallLog) -> Self {
        Self {
            log,
            applied: Mutex::new(BTreeMap::new()),
            fail_apply: Mutex::new(None),
            fail_destroy: Mutex::new(None),
            on_apply: Mutex::new(None),
            applies: Mutex::new(Vec::new()),
            observe: Mutex::new(None),
            status_during_apply: Mutex::new(Vec::new()),
            status_during_destroy: Mutex::new(Vec::new()),
            destroy_inputs: Mutex::new(Vec::new()),
        }
    }
}

#[async_trait]
impl SandboxProvider for FakeSandbox {
    async fn plan(&self, i: &SandboxInput) -> PortResult<SandboxPlan> {
        self.log.push(format!("sandbox.plan:{}", i.sandbox_id));
        let exists = lock(&self.applied).contains_key(&i.sandbox_id);
        Ok(SandboxPlan {
            changes: vec![PlanChange {
                action: if exists {
                    PlanAction::NoOp
                } else {
                    PlanAction::Create
                },
                address: "aws_s3_bucket.artifacts".into(),
                resource_type: "aws_s3_bucket".into(),
            }],
            summary: if exists {
                "Plan: 0 to add, 0 to change, 0 to destroy.".into()
            } else {
                "Plan: 1 to add, 0 to change, 0 to destroy.".into()
            },
        })
    }

    async fn apply(&self, i: &SandboxInput) -> PortResult<SandboxOutputs> {
        self.log.push(format!("sandbox.apply:{}", i.sandbox_id));
        lock(&self.applies).push(i.clone());
        let observe = lock(&self.observe).clone();
        if let Some(l) = observe
            && let Ok(Some(row)) = l.get_sandbox(&i.sandbox_id).await
        {
            lock(&self.status_during_apply).push(row.status.to_string());
        }
        let hook = lock(&self.on_apply).clone();
        if let Some(h) = hook {
            h(i);
        }
        if let Some(e) = lock(&self.fail_apply).clone() {
            return Err(e);
        }
        let out = ratchet_core::ports::mock::sample_sandbox_outputs(&i.sandbox_id);
        lock(&self.applied).insert(i.sandbox_id.clone(), out.clone());
        Ok(out)
    }

    async fn destroy(&self, i: &SandboxInput) -> PortResult<()> {
        self.log.push(format!("sandbox.destroy:{}", i.sandbox_id));
        lock(&self.destroy_inputs).push(i.clone());
        let observe = lock(&self.observe).clone();
        if let Some(l) = observe
            && let Ok(Some(row)) = l.get_sandbox(&i.sandbox_id).await
        {
            lock(&self.status_during_destroy).push(row.status.to_string());
        }
        if let Some(e) = lock(&self.fail_destroy).clone() {
            return Err(e);
        }
        lock(&self.applied).remove(&i.sandbox_id);
        Ok(())
    }

    async fn outputs(&self, id: &str) -> PortResult<Option<SandboxOutputs>> {
        Ok(lock(&self.applied).get(id).cloned())
    }
}

// ---------------------------------------------------------------- harness

pub const CONFIG_YAML: &str = "version: 0
objective:
  metric: latency_ms
  direction: minimize
  min_gain: 0.03
bench:
  ref: ratchet/bench
  run: bench.sh
  repeats: 3
  hardware: c7i.2xlarge
  confirm_trace: s3://sealed/trace.jsonl
guardrails:
  - run: pytest -q
  - peak_rss: \"< 1.2x\"
protected:
  - bench/**
  - ratchet.yaml
agent:
  instructions: RATCHET.md
  allowed_instance_types: [c7i.xlarge]
";

pub const TICK_MS: i64 = 60_000;

#[derive(Default)]
pub struct HarnessOptions {
    /// Also give the engine a local runner control (`h.local_runners`); optimizations then default to local agents.
    pub local_runners: bool,
    pub ledger_path: Option<PathBuf>,
    /// USD per hour of every instance type (default 1).
    pub hourly: Option<f64>,
    pub settings: EngineSettings,
    pub config_yaml: Option<String>,
    /// Billing source for `reconcile_costs` (omitted: the engine has none).
    pub costs: Option<Arc<dyn CostReconciler>>,
    pub mode: Option<Mode>,
    /// Wrap the manual clock the engine is given (e.g. to count sleeps); the ledger and the harness keep the plain one.
    pub wrap_clock: Option<Arc<dyn Fn(Arc<ManualClock>) -> Arc<dyn Clock> + Send + Sync>>,
}

#[derive(Default, Clone)]
pub struct BeginOptions {
    pub agents: Option<Vec<AgentKind>>,
    pub budget_usd: Option<f64>,
    pub time_limit_seconds: Option<u64>,
    pub plateau_evals: Option<u32>,
    pub baseline: Option<Vec<f64>>,
    pub confirm_baseline: Option<Vec<f64>>,
    pub sandbox_id: Option<String>,
    pub agents_where: Option<AgentsWhere>,
}

pub struct Harness {
    pub log: CallLog,
    pub clock: Arc<ManualClock>,
    pub ledger: Ledger,
    pub git: Arc<FakeGit>,
    pub bench: Arc<FakeBench>,
    pub runners: Arc<FakeRunners>,
    pub local_runners: Arc<FakeRunners>,
    pub compute: Arc<FakeCompute>,
    pub sandbox: Arc<FakeSandbox>,
    pub prices: Arc<FakePrices>,
    pub artifacts: Arc<FakeArtifacts>,
    pub engine: Engine,
    pub settings: EngineSettings,
    pub tmp: tempfile::TempDir,
    engine_clock: Arc<dyn Clock>,
    rx: broadcast::Receiver<EngineEvent>,
    seen: Vec<EngineEvent>,
    opts_local: bool,
    opts_mode: Mode,
    ledger_path: Option<PathBuf>,
    costs: Option<Arc<dyn CostReconciler>>,
}

#[derive(Debug, Clone)]
pub struct Begun {
    pub id: String,
    /// kind -> agent id
    pub agents: BTreeMap<AgentKind, String>,
}

impl Begun {
    pub fn agent(&self, kind: AgentKind) -> String {
        self.agents[&kind].clone()
    }
}

impl Harness {
    pub async fn new(opts: HarnessOptions) -> Harness {
        let log = CallLog::default();
        let clock = fake_clock();
        let ledger = open_ledger(opts.ledger_path.as_deref(), clock.clone()).await;
        let git = Arc::new(FakeGit::new(
            log.clone(),
            clock.clone(),
            &[
                (
                    "ratchet.yaml",
                    opts.config_yaml.as_deref().unwrap_or(CONFIG_YAML),
                ),
                ("RATCHET.md", "# Brief\nMake it faster.\n"),
            ],
        ));
        let mut settings = opts.settings;
        settings.tick_interval_ms.get_or_insert(TICK_MS as u64);
        settings
            .secrets
            .get_or_insert_with(|| Arc::new(|_k, name| Some(format!("value-of-{name}"))));
        settings
            .new_id
            .get_or_insert_with(|| Arc::new(|| "opt-test".to_owned()));
        let tmp = tempfile::tempdir().expect("tempdir");
        if settings.scratch_dir.is_none() {
            settings.scratch_dir = Some(tmp.path().join("scratch"));
        }
        let mut h = Harness {
            bench: Arc::new(FakeBench::new(log.clone(), clock.clone())),
            runners: Arc::new(FakeRunners::new(log.clone())),
            local_runners: Arc::new(FakeRunners::new(log.clone())),
            compute: Arc::new(FakeCompute::new(log.clone(), clock.clone())),
            sandbox: Arc::new(FakeSandbox::new(log.clone())),
            prices: Arc::new(FakePrices::new(opts.hourly.unwrap_or(1.0))),
            artifacts: Arc::new(FakeArtifacts::default()),
            engine_clock: match &opts.wrap_clock {
                Some(w) => w(clock.clone()),
                None => clock.clone(),
            },
            log,
            clock,
            git,
            settings,
            tmp,
            ledger: ledger.clone(),
            engine: placeholder_engine(&ledger).await,
            rx: broadcast::channel(1).1,
            seen: Vec::new(),
            opts_local: opts.local_runners,
            opts_mode: opts.mode.unwrap_or(Mode::Simulation),
            ledger_path: opts.ledger_path,
            costs: opts.costs,
        };
        h.engine = h.build().await;
        h.rx = h.engine.subscribe();
        h
    }

    async fn build(&self) -> Engine {
        Engine::new(EngineOptions {
            ledger: self.ledger.clone(),
            mode: self.opts_mode,
            clock: self.engine_clock.clone(),
            ports: EnginePorts {
                sandbox: self.sandbox.clone(),
                compute: self.compute.clone(),
                prices: self.prices.clone(),
                artifacts: self.artifacts.clone(),
                git: GitSource::One(self.git.clone()),
                runners: self.runners.clone(),
                local_runners: self
                    .opts_local
                    .then(|| self.local_runners.clone() as Arc<dyn RunnerControl>),
                bench: self.bench.clone(),
                costs: self.costs.clone(),
                environment: None,
            },
            settings: self.settings.clone(),
        })
        .await
    }

    /// A fresh `Engine` on the same fakes: a process restart. `reopen_ledger` also closes and reopens the SQLite file.
    pub async fn restart(&mut self, reopen_ledger: bool) -> Engine {
        self.engine.close().await;
        if reopen_ledger {
            let path = self
                .ledger_path
                .clone()
                .expect("reopen_ledger needs ledger_path");
            self.ledger.close().await;
            self.ledger = open_ledger(Some(&path), self.clock.clone()).await;
        }
        self.engine = self.build().await;
        self.rx = self.engine.subscribe();
        self.engine.clone()
    }

    /// Every engine event since the last call (of the current engine).
    pub fn events(&mut self) -> Vec<EngineEvent> {
        loop {
            match self.rx.try_recv() {
                Ok(e) => self.seen.push(e),
                Err(broadcast::error::TryRecvError::Lagged(_)) => continue,
                Err(_) => break,
            }
        }
        self.seen.clone()
    }

    /// Advance the clock one tick, tick, and wait for the bench job started (if any).
    pub async fn cycle(&self, n: usize) {
        for _ in 0..n {
            self.clock.advance(TICK_MS);
            self.engine.tick().await.expect("tick");
            self.engine.settle().await;
        }
    }

    pub async fn until<F>(&self, mut pred: F, max: usize)
    where
        F: AsyncFnMut() -> bool,
    {
        for _ in 0..max {
            if pred().await {
                return;
            }
            self.cycle(1).await;
            // background work (a resumed kill switch) runs on real I/O, not microtasks: give it real time
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        assert!(pred().await, "condition not met after {max} cycles");
    }

    /// Create + start an optimization.
    pub async fn begin(&self, b: BeginOptions) -> Result<Begun, crate::errors::EngineError> {
        self.bench.set(
            "main-sha",
            "baseline",
            ScriptedBench::samples(
                &b.baseline
                    .clone()
                    .unwrap_or_else(|| vec![100.0, 102.0, 98.0]),
            )
            .with_metric("peak_rss", 500.0),
        );
        self.bench.set(
            "main-sha",
            "confirm",
            ScriptedBench::samples(
                &b.confirm_baseline
                    .clone()
                    .unwrap_or_else(|| vec![100.0, 100.0, 100.0]),
            ),
        );
        let id = self
            .engine
            .create_optimization(CreateOptimizationInput {
                repo_url: self.git.remote_url(),
                base_ref: "main".into(),
                agents: b
                    .agents
                    .clone()
                    .unwrap_or_else(|| vec![AgentKind::ClaudeCode, AgentKind::Codex]),
                budget_usd: b.budget_usd.unwrap_or(100.0),
                time_limit_seconds: b.time_limit_seconds.unwrap_or(8 * 3600),
                plateau_evals: b.plateau_evals.unwrap_or(50),
                sandbox_id: b.sandbox_id.clone(),
                agents_where: b.agents_where,
                ..Default::default()
            })
            .await?;
        self.engine.start(&id).await?;
        let agents = self
            .ledger
            .list_agents(&OptimizationId::new(&id))
            .await
            .expect("agents")
            .into_iter()
            .map(|a| (a.kind, a.id.to_string()))
            .collect();
        Ok(Begun { id, agents })
    }

    /// Verdicts by sha.
    pub async fn verdicts(&self, id: &str) -> BTreeMap<String, String> {
        self.ledger
            .candidates_with_results(&OptimizationId::new(id))
            .await
            .expect("candidates")
            .into_iter()
            .map(|c| (c.candidate.sha, c.candidate.verdict.to_string()))
            .collect()
    }

    pub async fn status(&self, id: &str) -> ratchet_core::enums::OptimizationStatus {
        self.ledger
            .get_optimization(&OptimizationId::new(id))
            .await
            .expect("opt")
            .expect("exists")
            .status
    }

    /// Script a candidate: the commit on the branch plus its trusted bench / confirm samples.
    pub fn candidate(&self, branch: &str, sha: &str, c: Cand) {
        self.git.push(
            branch,
            sha,
            &c.files.iter().map(String::as_str).collect::<Vec<_>>(),
        );
        if let Some(bench) = c.bench {
            let g = if c.guardrail_fail {
                vec![failed("pytest -q", Some("exited with status 1"))]
            } else {
                vec![passed("pytest -q")]
            };
            self.bench.set(
                sha,
                "bench",
                ScriptedBench::samples(&bench).with_guardrails(g),
            );
        }
        if let Some(confirm) = c.confirm {
            self.bench
                .set(sha, "confirm", ScriptedBench::samples(&confirm));
        }
    }
}

/// Options of [`Harness::candidate`].
pub struct Cand {
    pub files: Vec<String>,
    pub bench: Option<Vec<f64>>,
    pub confirm: Option<Vec<f64>>,
    pub guardrail_fail: bool,
}

impl Cand {
    pub fn new() -> Self {
        Self {
            files: vec!["src/search.ts".into()],
            bench: None,
            confirm: None,
            guardrail_fail: false,
        }
    }

    pub fn bench(mut self, s: &[f64]) -> Self {
        self.bench = Some(s.to_vec());
        self
    }

    pub fn confirm(mut self, s: &[f64]) -> Self {
        self.confirm = Some(s.to_vec());
        self
    }

    pub fn files(mut self, f: &[&str]) -> Self {
        self.files = f.iter().map(|s| (*s).to_owned()).collect();
        self
    }

    pub fn guardrail_fail(mut self) -> Self {
        self.guardrail_fail = true;
        self
    }
}

impl Default for Cand {
    fn default() -> Self {
        Self::new()
    }
}

/// bench + confirm with a single value repeated three times (the common case).
pub fn win(v: f64) -> Cand {
    Cand::new().bench(&[v, v, v]).confirm(&[v, v, v])
}

pub fn ev(event_type: ratchet_core::ports::AgentEventType) -> AgentEvent {
    AgentEvent::new(1, event_type)
}

pub fn status_event(data: serde_json::Value) -> AgentEvent {
    ev(ratchet_core::ports::AgentEventType::Status).with_data(data)
}

async fn open_ledger(path: Option<&Path>, clock: Arc<ManualClock>) -> Ledger {
    let options = LedgerOptions {
        clock: Some(clock),
        max_connections: None,
    };
    match path {
        Some(p) => Ledger::open(p, options).await.expect("open ledger"),
        None => Ledger::open_memory(options).await.expect("memory ledger"),
    }
}

async fn placeholder_engine(ledger: &Ledger) -> Engine {
    // replaced right after construction; needs real ports only to exist
    let log = CallLog::default();
    let clock = fake_clock();
    Engine::new(EngineOptions {
        ledger: ledger.clone(),
        mode: Mode::Simulation,
        clock: clock.clone(),
        ports: EnginePorts {
            sandbox: Arc::new(FakeSandbox::new(log.clone())),
            compute: Arc::new(FakeCompute::new(log.clone(), clock.clone())),
            prices: Arc::new(FakePrices::new(1.0)),
            artifacts: Arc::new(FakeArtifacts::default()),
            git: GitSource::One(Arc::new(FakeGit::new(log.clone(), clock.clone(), &[]))),
            runners: Arc::new(FakeRunners::new(log.clone())),
            local_runners: None,
            bench: Arc::new(FakeBench::new(log, clock)),
            costs: None,
            environment: None,
        },
        settings: EngineSettings::default(),
    })
    .await
}
