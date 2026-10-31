use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::Mutex;

use crate::artifacts::{config_key, inbox_key, last_seq, read_events, runner_prefix};
use crate::domain::{ArtifactStore, CloudCompute, ControlError, PollResult, RemoteCommand, RunnerConfig, RunnerControl, RunnerHandle, RunnerMessage};

pub struct SsmRunnerControlOptions {
    pub compute: Arc<dyn CloudCompute>,
    /// The artifacts bucket (keys are `artifacts/<opt>/<agent>/...`).
    pub store: Arc<dyn ArtifactStore>,
    /// Executable the AMI installs. Default `ratchet-runner`.
    pub runner_command: Option<String>,
    /// Extra environment for the runner process (`RATCHET_ARTIFACTS_BUCKET`, `RATCHET_SECRET_REFS`, `AWS_REGION`, ...).
    pub runner_env: BTreeMap<String, String>,
    /// Directory on the instance for pid and log files. Default `/var/lib/ratchet`.
    pub state_dir: Option<String>,
}

/// Single-quote a value for a POSIX shell.
fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Live-mode `RunnerControl`: the engine never talks to the runner directly.
///  - start: config as `config.json` in the artifacts store, then an SSM command launches `ratchet-runner` detached
///  - send:  numbered objects `inbox/00000001.json`
///  - poll:  numbered JSONL batches `events/00000001.jsonl`, cursor = highest batch read
///  - stop:  a `stop` message plus SIGTERM to the runner pid (the runner aborts its agent's process group)
pub struct SsmRunnerControl {
    options: SsmRunnerControlOptions,
    next_inbox: Mutex<HashMap<String, u64>>,
    runner_command: String,
    state_dir: String,
}

impl SsmRunnerControl {
    pub fn new(options: SsmRunnerControlOptions) -> Self {
        let runner_command = options.runner_command.clone().unwrap_or_else(|| "ratchet-runner".to_string());
        let state_dir = options.state_dir.clone().unwrap_or_else(|| "/var/lib/ratchet".to_string());
        Self { options, next_inbox: Mutex::new(HashMap::new()), runner_command, state_dir }
    }

    fn key(h: &RunnerHandle) -> String {
        format!("{}/{}", h.optimization_id, h.agent_id)
    }

    fn prefix(h: &RunnerHandle) -> String {
        runner_prefix(&h.optimization_id, &h.agent_id)
    }

    fn pid_file(&self, h: &RunnerHandle) -> String {
        format!("{}/{}-{}.pid", self.state_dir, h.optimization_id, h.agent_id)
    }
}

#[async_trait]
impl RunnerControl for SsmRunnerControl {
    async fn start(&self, handle: &RunnerHandle, config: &RunnerConfig) -> Result<(), ControlError> {
        let prefix = Self::prefix(handle);
        let body = serde_json::to_string(config).map_err(|e| ControlError::new(e.to_string()))?;
        self.options.store.put(&config_key(&prefix), body.as_bytes()).await.map_err(|e| ControlError::new(e.to_string()))?;
        let mut env: Vec<(String, String)> = vec![
            ("RATCHET_OPTIMIZATION_ID".to_string(), handle.optimization_id.clone()),
            ("RATCHET_AGENT_ID".to_string(), handle.agent_id.clone()),
        ];
        for (k, v) in &self.options.runner_env {
            match env.iter_mut().find(|(ek, _)| ek == k) {
                Some(slot) => slot.1 = v.clone(),
                None => env.push((k.clone(), v.clone())),
            }
        }
        let assignments = env.iter().map(|(k, v)| format!("{k}={}", q(v))).collect::<Vec<_>>().join(" ");
        let log = format!("{}/{}-{}.log", self.state_dir, handle.optimization_id, handle.agent_id);
        let pid_file = self.pid_file(handle);
        let script = [
            "set -eu".to_string(),
            format!("mkdir -p {}", q(&self.state_dir)),
            format!("if [ -f {pid} ] && kill -0 \"$(cat {pid})\" 2>/dev/null; then echo \"runner already running\" >&2; exit 1; fi", pid = q(&pid_file)),
            format!("setsid nohup env {assignments} {} > {} 2>&1 < /dev/null &", self.runner_command, q(&log)),
            format!("echo $! > {}", q(&pid_file)),
        ]
        .join("\n");
        let res = self.options.compute.run_command(&handle.instance_id, RemoteCommand { script, timeout_seconds: Some(60), ..RemoteCommand::default() }).await?;
        if res.exit_code != 0 {
            let detail = if res.stderr.trim().is_empty() { res.stdout.trim() } else { res.stderr.trim() };
            return Err(ControlError::new(format!("failed to start runner on {}: {detail}", handle.instance_id)));
        }
        Ok(())
    }

    async fn send(&self, handle: &RunnerHandle, message: RunnerMessage) -> Result<(), ControlError> {
        let prefix = Self::prefix(handle);
        let k = Self::key(handle);
        let n = {
            let mut next = self.next_inbox.lock().await;
            let n = match next.get(&k) {
                Some(n) => *n,
                None => last_seq(self.options.store.as_ref(), &format!("{prefix}inbox/")).await.map_err(|e| ControlError::new(e.to_string()))? + 1,
            };
            next.insert(k, n + 1);
            n
        };
        let body = serde_json::to_string(&message).map_err(|e| ControlError::new(e.to_string()))?;
        self.options.store.put(&inbox_key(&prefix, n), body.as_bytes()).await.map_err(|e| ControlError::new(e.to_string()))
    }

    async fn poll(&self, handle: &RunnerHandle, cursor: Option<&str>) -> Result<PollResult, ControlError> {
        read_events(self.options.store.as_ref(), &Self::prefix(handle), cursor).await.map_err(|e| ControlError::new(e.to_string()))
    }

    async fn stop(&self, handle: &RunnerHandle) -> Result<(), ControlError> {
        self.send(handle, RunnerMessage::Stop).await?;
        let pid = q(&self.pid_file(handle));
        let script = format!("if [ -f {pid} ]; then kill -TERM \"$(cat {pid})\" 2>/dev/null || true; fi");
        // best effort: the instance may already be gone (kill switch), and the stop message is durable
        let _ = self.options.compute.run_command(&handle.instance_id, RemoteCommand { script, timeout_seconds: Some(30), ..RemoteCommand::default() }).await;
        Ok(())
    }
}
