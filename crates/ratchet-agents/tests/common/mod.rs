//! Shared helpers for the integration tests: git sandboxes, a fake engine that answers commits with verdicts, polling.
#![allow(dead_code)]

use ratchet_core::enums::CandidateVerdict;
use ratchet_core::ids::{AgentId, OptimizationId};
use ratchet_core::ports::{
    AgentEvent, AgentEventType, AgentIdentity, ArtifactStore, BenchLaunchSpec, CloudCompute,
    InstanceInfo, PortError, PortResult, RemoteCommand, RemoteCommandResult, RunnerConfig,
    RunnerLaunchSpec, RunnerMessage, SecretRef,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;

use serde_json::json;

pub const SEED_FILES: [(&str, &str); 3] = [
    ("src/app.txt", "line 1\nline 2\nline 3\n"),
    ("bench/run.sh", "echo bench\n"),
    ("README.md", "# demo\n"),
];

pub fn sh(cwd: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_AUTHOR_NAME", "t")
        .env("GIT_AUTHOR_EMAIL", "t@t")
        .env("GIT_COMMITTER_NAME", "t")
        .env("GIT_COMMITTER_EMAIL", "t@t")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?} in {cwd:?} failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub struct Sandbox {
    pub tmp: tempfile::TempDir,
    pub dir: PathBuf,
    pub remote: PathBuf,
    /// A scratch clone for the "engine" side (updating ratchet/best, reading branches).
    pub engine: PathBuf,
}

impl Sandbox {
    pub fn remote_url(&self) -> String {
        self.remote.to_string_lossy().into_owned()
    }
}

pub fn write_file(dir: &Path, path: &str, content: &str) {
    let full = dir.join(path);
    std::fs::create_dir_all(full.parent().unwrap()).unwrap();
    std::fs::write(full, content).unwrap();
}

/// A bare remote with `main` seeded from `files`, plus agent branches off main.
pub fn make_sandbox_with(files: &[(&str, &str)], branches: &[&str]) -> Sandbox {
    let tmp = tempfile::Builder::new()
        .prefix("ratchet-test-")
        .tempdir()
        .unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let remote = dir.join("remote.git");
    let seed = dir.join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    let ok = Command::new("git")
        .args(["init", "--quiet", "--bare", "--initial-branch=main"])
        .arg(&remote)
        .status()
        .unwrap();
    assert!(ok.success());
    sh(&seed, &["init", "--quiet", "--initial-branch=main"]);
    for (path, content) in files {
        write_file(&seed, path, content);
    }
    sh(&seed, &["add", "-A"]);
    sh(&seed, &["commit", "--quiet", "-m", "seed"]);
    sh(
        &seed,
        &["remote", "add", "origin", &remote.to_string_lossy()],
    );
    sh(&seed, &["push", "--quiet", "origin", "main"]);
    for b in branches {
        sh(
            &seed,
            &["push", "--quiet", "origin", &format!("main:refs/heads/{b}")],
        );
    }
    let engine = dir.join("engine");
    let ok = Command::new("git")
        .args(["clone", "--quiet"])
        .arg(&remote)
        .arg(&engine)
        .status()
        .unwrap();
    assert!(ok.success());
    Sandbox {
        tmp,
        dir,
        remote,
        engine,
    }
}

pub fn make_sandbox() -> Sandbox {
    make_sandbox_with(&SEED_FILES, &["ratchet/a"])
}

pub fn make_sandbox_branches(branches: &[&str]) -> Sandbox {
    make_sandbox_with(&SEED_FILES, branches)
}

/// Engine side: point `ratchet/best` at `sha` (fetching it from the remote first).
pub fn set_best(sb: &Sandbox, sha: &str) {
    sh(&sb.engine, &["fetch", "--quiet", "origin"]);
    sh(
        &sb.engine,
        &[
            "push",
            "--quiet",
            "--force",
            "origin",
            &format!("{sha}:refs/heads/ratchet/best"),
        ],
    );
}

/// The runner config used by every test; `over` can tweak the JSON before it is decoded.
pub fn base_config_with(remote: &str, over: impl FnOnce(&mut serde_json::Value)) -> RunnerConfig {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_millis() as i64;
    let evaluator = json!({"kind": "benchmark", "ref": "ratchet/bench", "run": "k6 run bench/k6/search.js", "repeats": 5, "hardware": "c7i.2xlarge", "deps": "auto"});
    let mut value = json!({
        "optimizationId": "opt1",
        "agentId": "a",
        "kind": "mock",
        "repoUrl": remote,
        "branch": "ratchet/a",
        "baseRef": "main",
        "config": {
            "version": 0,
            "objective": {"metric": "http_req_duration.p95", "direction": "minimize", "min_gain": 0.03},
            "evaluator": evaluator.clone(),
            "bench": evaluator,
            "guardrails": [
                {"kind": "command", "run": "pnpm test"},
                {"kind": "threshold", "metric": "error_rate", "op": "<", "value": 0.001, "relative": false, "raw": "< 0.001"}
            ],
            "protected": ["bench/**", "ratchet.yaml"],
            "agent": {"instructions": "RATCHET.md", "allowed_instance_types": ["c7i.2xlarge"]}
        },
        "brief": "# Brief\n\nMake it faster. Do not touch bench/**.\n",
        "baselineValue": 20,
        "budgetUsd": 50,
        "deadlineAt": now + 60_000
    });
    over(&mut value);
    serde_json::from_value(value).expect("test config decodes")
}

pub fn base_config(sb: &Sandbox, over: impl FnOnce(&mut serde_json::Value)) -> RunnerConfig {
    base_config_with(&sb.remote_url(), over)
}

#[derive(Debug, Clone)]
pub struct Decision {
    pub verdict: CandidateVerdict,
    pub reasons: Vec<String>,
    pub best: Option<f64>,
}

impl Decision {
    pub fn new(verdict: CandidateVerdict) -> Self {
        Self {
            verdict,
            reasons: Vec::new(),
            best: None,
        }
    }
    pub fn with_reasons(mut self, reasons: &[&str]) -> Self {
        self.reasons = reasons.iter().map(|r| r.to_string()).collect();
        self
    }
}

pub struct CommitInfo {
    pub sha: String,
    pub files: Vec<String>,
    pub n: usize,
}

pub type VerdictRule = Arc<dyn Fn(&CommitInfo) -> Option<Decision> + Send + Sync>;

pub fn rule(f: impl Fn(&CommitInfo) -> Option<Decision> + Send + Sync + 'static) -> VerdictRule {
    Arc::new(f)
}

/// Called with the sha and the verdict just before the verdict is delivered.
pub type VerdictHook = Arc<dyn Fn(&str, CandidateVerdict) + Send + Sync>;

#[derive(Default)]
pub struct AutoOptions {
    pub rule: Option<VerdictRule>,
    pub start: Option<f64>,
    pub on_verdict: Option<VerdictHook>,
}

/// Plays the engine: answers every `commit` event with a verdict (default: verified, best improves 2 per win). `on_verdict` runs
/// before the verdict is delivered (e.g. to move ratchet/best).
pub fn auto_verdicts(
    push: impl Fn(RunnerMessage) + Send + Sync + 'static,
    opts: AutoOptions,
) -> Arc<dyn Fn(&AgentEvent) + Send + Sync> {
    let state = Mutex::new((opts.start.unwrap_or(20.0), 0usize));
    Arc::new(move |e: &AgentEvent| {
        if e.event_type != AgentEventType::Commit {
            return;
        }
        let sha = e.data_str("sha").unwrap_or_default().to_string();
        let files: Vec<String> = e
            .data
            .as_ref()
            .and_then(|d| d.get("files"))
            .and_then(|f| f.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(str::to_string))
                    .collect()
            })
            .unwrap_or_default();
        let (decision, best) = {
            let mut st = state.lock().unwrap();
            st.1 += 1;
            let info = CommitInfo {
                sha: sha.clone(),
                files,
                n: st.1,
            };
            let decision = opts
                .rule
                .as_ref()
                .and_then(|r| r(&info))
                .unwrap_or_else(|| Decision::new(CandidateVerdict::Verified));
            if decision.verdict == CandidateVerdict::Verified {
                st.0 = decision.best.unwrap_or(st.0 - 2.0);
            }
            (decision, st.0)
        };
        if let Some(cb) = &opts.on_verdict {
            cb(&sha, decision.verdict);
        }
        push(RunnerMessage::Verdict {
            sha,
            verdict: decision.verdict,
            reasons: decision.reasons,
            current_best: best,
        });
    })
}

pub async fn until<F: FnMut() -> bool>(mut cond: F, what: &str) {
    until_for(&mut cond, Duration::from_secs(10), what).await
}

pub async fn until_for<F: FnMut() -> bool>(cond: &mut F, limit: Duration, what: &str) {
    let start = Instant::now();
    while !cond() {
        assert!(start.elapsed() <= limit, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

pub fn fixture(name: &str) -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name),
    )
    .expect("fixture exists")
}

pub fn collect() -> (
    Arc<Mutex<Vec<AgentEvent>>>,
    impl Fn(AgentEvent) + Send + Sync + 'static,
) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    (events, move |e| sink.lock().unwrap().push(e))
}

pub fn texts(events: &[AgentEvent]) -> Vec<String> {
    events
        .iter()
        .map(|e| e.text.clone().unwrap_or_default())
        .collect()
}

// ---------------------------------------------------------------- in-memory capabilities

#[derive(Default)]
pub struct MemoryStore {
    pub objects: Mutex<std::collections::BTreeMap<String, String>>,
}

#[async_trait]
impl ArtifactStore for MemoryStore {
    async fn put(&self, key: &str, body: &[u8]) -> PortResult<()> {
        self.objects
            .lock()
            .unwrap()
            .insert(key.to_string(), String::from_utf8_lossy(body).into_owned());
        Ok(())
    }
    async fn get(&self, key: &str) -> PortResult<Option<String>> {
        Ok(self.objects.lock().unwrap().get(key).cloned())
    }
    async fn list(&self, prefix: &str) -> PortResult<Vec<String>> {
        Ok(self
            .objects
            .lock()
            .unwrap()
            .keys()
            .filter(|k| k.starts_with(prefix))
            .cloned()
            .collect())
    }
}

impl MemoryStore {
    pub fn keys(&self) -> Vec<String> {
        self.objects.lock().unwrap().keys().cloned().collect()
    }
    pub fn text(&self, key: &str) -> Option<String> {
        self.objects.lock().unwrap().get(key).cloned()
    }
}

pub struct FakeCompute {
    pub commands: Mutex<Vec<(String, RemoteCommand)>>,
    pub next_result: Mutex<RemoteCommandResult>,
    pub fail: Mutex<bool>,
}

impl Default for FakeCompute {
    fn default() -> Self {
        Self {
            commands: Mutex::new(Vec::new()),
            next_result: Mutex::new(RemoteCommandResult::default()),
            fail: Mutex::new(false),
        }
    }
}

/// Only the SSM command channel works; the rest of the port is not used by the runner control.
#[async_trait]
impl CloudCompute for FakeCompute {
    async fn create_agent_identity(
        &self,
        _: &OptimizationId,
        _: &AgentId,
    ) -> PortResult<AgentIdentity> {
        Err(PortError::Unsupported("create_agent_identity"))
    }
    async fn revoke_agent_identity(&self, _: &AgentIdentity) -> PortResult<()> {
        Err(PortError::Unsupported("revoke_agent_identity"))
    }
    async fn delete_agent_identity(&self, _: &AgentIdentity) -> PortResult<()> {
        Err(PortError::Unsupported("delete_agent_identity"))
    }
    async fn put_secret(
        &self,
        _: &OptimizationId,
        _: &AgentId,
        _: &str,
        _: &str,
    ) -> PortResult<SecretRef> {
        Err(PortError::Unsupported("put_secret"))
    }
    async fn delete_secrets(&self, _: &OptimizationId, _: &AgentId) -> PortResult<()> {
        Err(PortError::Unsupported("delete_secrets"))
    }
    async fn launch_runner(&self, _: &RunnerLaunchSpec) -> PortResult<InstanceInfo> {
        Err(PortError::Unsupported("launch_runner"))
    }
    async fn launch_bench_instance(&self, _: &BenchLaunchSpec) -> PortResult<InstanceInfo> {
        Err(PortError::Unsupported("launch_bench_instance"))
    }
    async fn terminate(&self, _: &[String]) -> PortResult<()> {
        Err(PortError::Unsupported("terminate"))
    }
    async fn list_tagged(&self, _: Option<&OptimizationId>) -> PortResult<Vec<InstanceInfo>> {
        Err(PortError::Unsupported("list_tagged"))
    }
    async fn run_command(
        &self,
        instance_id: &str,
        command: &RemoteCommand,
    ) -> PortResult<RemoteCommandResult> {
        self.commands
            .lock()
            .unwrap()
            .push((instance_id.to_string(), command.clone()));
        if *self.fail.lock().unwrap() {
            return Err(PortError::failed("instance gone"));
        }
        Ok(self.next_result.lock().unwrap().clone())
    }
}

pub fn env_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

/// `AgentSpec::env` is ordered.
pub fn btree_env_of(pairs: &[(&str, &str)]) -> std::collections::BTreeMap<String, String> {
    pairs
        .iter()
        .map(|(k, v)| (k.to_string(), v.to_string()))
        .collect()
}

pub fn path_env() -> String {
    std::env::var("PATH").unwrap_or_default()
}

// ---------------------------------------------------------------- the demo repositories under examples/

/// The Ratchet repository root (the demos still live in `examples/` until the TypeScript packages are retired).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("repo root")
}

/// Recursive copy that leaves out `node_modules` and `dist`.
pub fn copy_demo_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap().flatten() {
        let name = entry.file_name();
        if name == "node_modules" || name == "dist" {
            continue;
        }
        let target = to.join(&name);
        if entry.path().is_dir() {
            copy_demo_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// A throwaway git tree of `examples/<demo>`.
pub fn scratch_demo_tree(demo: &str) -> tempfile::TempDir {
    let dir = tempfile::Builder::new()
        .prefix("ratchet-demo-scratch-")
        .tempdir()
        .unwrap();
    copy_demo_tree(&repo_root().join("examples").join(demo), dir.path());
    sh(dir.path(), &["init", "-q"]);
    dir
}

/// `git apply [--check] --whitespace=nowarn <patch>` in `dir`; true when it succeeded.
pub fn git_apply(dir: &Path, patch: &Path, check: bool) -> bool {
    let mut cmd = Command::new("git");
    cmd.arg("apply");
    if check {
        cmd.arg("--check");
    }
    cmd.arg("--whitespace=nowarn")
        .arg(patch)
        .current_dir(dir)
        .status()
        .unwrap()
        .success()
}

/// Names of the patch steps in play order.
pub fn patch_order(script: &ratchet_agents::AgentScript) -> Vec<String> {
    script
        .attempts
        .iter()
        .flat_map(|a| a.steps.iter())
        .filter_map(|s| match s {
            ratchet_agents::ScriptStep::Patch { name, .. } => name.clone(),
            _ => None,
        })
        .collect()
}

/// Every attempt carries realistic usage (the cost meter has something to count).
pub fn every_attempt_has_realistic_usage(script: &ratchet_agents::AgentScript) -> bool {
    script.attempts.iter().all(|a| {
        a.steps.iter().any(|s| matches!(s, ratchet_agents::ScriptStep::Usage { input_tokens, output_tokens, .. } if *input_tokens > 10_000 && *output_tokens > 100))
    })
}
