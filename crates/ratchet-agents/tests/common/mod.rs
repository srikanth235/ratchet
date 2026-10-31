//! Shared helpers for the integration tests: git sandboxes, a fake engine that answers commits with verdicts, polling.
#![allow(dead_code)]

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use ratchet_agents::domain::{
    AgentEvent, AgentEventType, ArtifactStore, CandidateVerdict, CloudCompute, ControlError, RemoteCommand, RemoteCommandResult, RunnerConfig,
    RunnerMessage, StoreError,
};
use serde_json::json;

pub const SEED_FILES: [(&str, &str); 3] = [("src/app.txt", "line 1\nline 2\nline 3\n"), ("bench/run.sh", "echo bench\n"), ("README.md", "# demo\n")];

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
    assert!(out.status.success(), "git {args:?} in {cwd:?} failed: {}", String::from_utf8_lossy(&out.stderr));
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
    let tmp = tempfile::Builder::new().prefix("ratchet-test-").tempdir().unwrap();
    let dir = tmp.path().canonicalize().unwrap();
    let remote = dir.join("remote.git");
    let seed = dir.join("seed");
    std::fs::create_dir_all(&seed).unwrap();
    let ok = Command::new("git").args(["init", "--quiet", "--bare", "--initial-branch=main"]).arg(&remote).status().unwrap();
    assert!(ok.success());
    sh(&seed, &["init", "--quiet", "--initial-branch=main"]);
    for (path, content) in files {
        write_file(&seed, path, content);
    }
    sh(&seed, &["add", "-A"]);
    sh(&seed, &["commit", "--quiet", "-m", "seed"]);
    sh(&seed, &["remote", "add", "origin", &remote.to_string_lossy()]);
    sh(&seed, &["push", "--quiet", "origin", "main"]);
    for b in branches {
        sh(&seed, &["push", "--quiet", "origin", &format!("main:refs/heads/{b}")]);
    }
    let engine = dir.join("engine");
    let ok = Command::new("git").args(["clone", "--quiet"]).arg(&remote).arg(&engine).status().unwrap();
    assert!(ok.success());
    Sandbox { tmp, dir, remote, engine }
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
    sh(&sb.engine, &["push", "--quiet", "--force", "origin", &format!("{sha}:refs/heads/ratchet/best")]);
}

/// The runner config used by every test; `over` can tweak the JSON before it is decoded.
pub fn base_config_with(remote: &str, over: impl FnOnce(&mut serde_json::Value)) -> RunnerConfig {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_millis() as i64;
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
        Self { verdict, reasons: Vec::new(), best: None }
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

#[derive(Default)]
pub struct AutoOptions {
    pub rule: Option<VerdictRule>,
    pub start: Option<f64>,
    pub on_verdict: Option<Arc<dyn Fn(&str, CandidateVerdict) + Send + Sync>>,
}

/// Plays the engine: answers every `commit` event with a verdict (default: verified, best improves 2 per win). `on_verdict` runs
/// before the verdict is delivered (e.g. to move ratchet/best).
pub fn auto_verdicts(push: impl Fn(RunnerMessage) + Send + Sync + 'static, opts: AutoOptions) -> Arc<dyn Fn(&AgentEvent) + Send + Sync> {
    let state = Mutex::new((opts.start.unwrap_or(20.0), 0usize));
    Arc::new(move |e: &AgentEvent| {
        if e.kind != AgentEventType::Commit {
            return;
        }
        let sha = e.data_str("sha").unwrap_or_default().to_string();
        let files: Vec<String> = e
            .data
            .as_ref()
            .and_then(|d| d.get("files"))
            .and_then(|f| f.as_array())
            .map(|a| a.iter().filter_map(|v| v.as_str().map(str::to_string)).collect())
            .unwrap_or_default();
        let (decision, best) = {
            let mut st = state.lock().unwrap();
            st.1 += 1;
            let info = CommitInfo { sha: sha.clone(), files, n: st.1 };
            let decision = opts.rule.as_ref().and_then(|r| r(&info)).unwrap_or_else(|| Decision::new(CandidateVerdict::Verified));
            if decision.verdict == CandidateVerdict::Verified {
                st.0 = decision.best.unwrap_or(st.0 - 2.0);
            }
            (decision, st.0)
        };
        if let Some(cb) = &opts.on_verdict {
            cb(&sha, decision.verdict);
        }
        push(RunnerMessage::Verdict { sha, verdict: decision.verdict, reasons: decision.reasons, current_best: best });
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
    std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures").join(name)).expect("fixture exists")
}

pub fn collect() -> (Arc<Mutex<Vec<AgentEvent>>>, impl Fn(AgentEvent) + Send + Sync + 'static) {
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = events.clone();
    (events, move |e| sink.lock().unwrap().push(e))
}

pub fn texts(events: &[AgentEvent]) -> Vec<String> {
    events.iter().map(|e| e.text.clone().unwrap_or_default()).collect()
}

// ---------------------------------------------------------------- in-memory capabilities

#[derive(Default)]
pub struct MemoryStore {
    pub objects: Mutex<std::collections::BTreeMap<String, String>>,
}

#[async_trait]
impl ArtifactStore for MemoryStore {
    async fn put(&self, key: &str, body: &[u8]) -> Result<(), StoreError> {
        self.objects.lock().unwrap().insert(key.to_string(), String::from_utf8_lossy(body).into_owned());
        Ok(())
    }
    async fn get(&self, key: &str) -> Result<Option<String>, StoreError> {
        Ok(self.objects.lock().unwrap().get(key).cloned())
    }
    async fn list(&self, prefix: &str) -> Result<Vec<String>, StoreError> {
        Ok(self.objects.lock().unwrap().keys().filter(|k| k.starts_with(prefix)).cloned().collect())
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
        Self { commands: Mutex::new(Vec::new()), next_result: Mutex::new(RemoteCommandResult::default()), fail: Mutex::new(false) }
    }
}

#[async_trait]
impl CloudCompute for FakeCompute {
    async fn run_command(&self, instance_id: &str, command: RemoteCommand) -> Result<RemoteCommandResult, ControlError> {
        self.commands.lock().unwrap().push((instance_id.to_string(), command));
        if *self.fail.lock().unwrap() {
            return Err(ControlError::new("instance gone"));
        }
        Ok(self.next_result.lock().unwrap().clone())
    }
}

pub fn env_of(pairs: &[(&str, &str)]) -> HashMap<String, String> {
    pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
}

pub fn path_env() -> String {
    std::env::var("PATH").unwrap_or_default()
}
