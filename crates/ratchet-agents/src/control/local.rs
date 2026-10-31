use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use futures::FutureExt;
use futures::future::{BoxFuture, Shared};
use serde_json::json;
use tokio::sync::{Mutex as AsyncMutex, OnceCell};
use tokio_util::sync::CancellationToken;

use crate::clock::SystemClock;
use crate::domain::{
    AgentAdapter, AgentEvent, AgentEventType, Clock, ControlError, PollResult, RunnerConfig,
    RunnerControl, RunnerHandle, RunnerMessage,
};
use crate::git::{Git, SharedGit};
use crate::inbox::MemoryInbox;
use crate::runner_loop::{
    LoopTuning, RunnerLoopOptions, RunnerLoopResult, RunnerStopReason, run_runner_loop,
};
use crate::workdir::{
    AddWorktreeOptions, Identity, LocalCloneOptions, PrepareWorkdirOptions, add_worktree,
    ensure_local_clone, prepare_workdir, remove_worktree,
};

/// Builds the adapter for one agent; an `Err` (for example a missing or signed-out CLI, with its hint) fails the launch.
pub type AdapterFor = Arc<
    dyn Fn(&RunnerConfig, &RunnerHandle) -> Result<Arc<dyn AgentAdapter>, String> + Send + Sync,
>;
/// Extra environment for the agent processes of one agent (passed to the loop as `agent_env`).
pub type AgentEnvFor =
    Arc<dyn Fn(&RunnerConfig, &RunnerHandle) -> Option<HashMap<String, String>> + Send + Sync>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum WorkdirMode {
    /// A full clone per agent (simulation).
    #[default]
    Clone,
    /// The app's "This machine" mode: one local clone per optimization under `<root>/<optimization>/clone`, and a `git worktree` of
    /// it per agent. The worktree is removed when the agent is stopped; the clone when the optimization is torn down (`release`).
    Worktree,
}

pub struct LocalRunnerControlOptions {
    /// Each agent works under `<root_dir>/<optimization_id>/`: `<agent_id>` (a clone, or a worktree of `clone/`).
    pub root_dir: PathBuf,
    pub adapter_for: AdapterFor,
    pub clock: Option<Arc<dyn Clock>>,
    pub git: Option<SharedGit>,
    pub git_env: Option<HashMap<String, String>>,
    /// Overrides for the loop (best_ref, max_idle_turns, retry_delay, ...). `git` and `git_env` above take precedence.
    pub loop_tuning: LoopTuning,
    /// Wait this long for a graceful stop before aborting the running turn. Default 10 s.
    pub stop_grace: Option<Duration>,
    pub workdirs: WorkdirMode,
    pub agent_env_for: Option<AgentEnvFor>,
}

impl LocalRunnerControlOptions {
    pub fn new(root_dir: impl Into<PathBuf>, adapter_for: AdapterFor) -> Self {
        Self {
            root_dir: root_dir.into(),
            adapter_for,
            clock: None,
            git: None,
            git_env: None,
            loop_tuning: LoopTuning::default(),
            stop_grace: None,
            workdirs: WorkdirMode::Clone,
            agent_env_for: None,
        }
    }
}

type Done = Shared<BoxFuture<'static, RunnerLoopResult>>;

struct LocalRunner {
    inbox: Arc<MemoryInbox>,
    events: Arc<Mutex<Vec<AgentEvent>>>,
    abort: CancellationToken,
    done: Done,
    workdir: PathBuf,
    stopped: AtomicBool,
    /// Worktree mode: the clone this runner's worktree belongs to.
    clone: Option<Git>,
}

fn key_of(h: &RunnerHandle) -> String {
    format!("{}/{}", h.optimization_id, h.agent_id)
}

/// `RunnerControl` that runs the runner loop in this process: in-memory inbox and event buffer, one working directory per agent.
///
/// Used with `ScriptedAgent` in simulation and with the real Claude Code / Codex adapters for local agents (`WorkdirMode::Worktree`,
/// the user's own CLI login). Agents do not survive the process: when the app restarts, `poll` reports each lost agent as failed so
/// the engine can end the run instead of waiting for events that will never come.
pub struct LocalRunnerControl {
    options: LocalRunnerControlOptions,
    runners: Mutex<HashMap<String, Arc<LocalRunner>>>,
    clones: Mutex<HashMap<String, Arc<OnceCell<Git>>>>,
    /// Serializes git writes per optimization (they lock the shared clone's metadata).
    queues: Mutex<HashMap<String, Arc<AsyncMutex<()>>>>,
    clock: Arc<dyn Clock>,
}

impl LocalRunnerControl {
    pub fn new(options: LocalRunnerControlOptions) -> Self {
        let clock = options
            .clock
            .clone()
            .unwrap_or_else(|| Arc::new(SystemClock));
        Self {
            options,
            runners: Mutex::new(HashMap::new()),
            clones: Mutex::new(HashMap::new()),
            queues: Mutex::new(HashMap::new()),
            clock,
        }
    }

    fn queue(&self, optimization_id: &str) -> Arc<AsyncMutex<()>> {
        self.queues
            .lock()
            .unwrap()
            .entry(optimization_id.to_string())
            .or_default()
            .clone()
    }

    fn clone_dir(&self, optimization_id: &str) -> PathBuf {
        self.options.root_dir.join(optimization_id).join("clone")
    }

    async fn clone_for(&self, optimization_id: &str, repo_url: &str) -> Result<Git, ControlError> {
        let cell = self
            .clones
            .lock()
            .unwrap()
            .entry(optimization_id.to_string())
            .or_default()
            .clone();
        // a failed clone leaves the cell empty, so the next start retries it
        cell.get_or_try_init(|| async {
            ensure_local_clone(LocalCloneOptions {
                repo_url: repo_url.to_string(),
                dir: self.clone_dir(optimization_id),
                git: self.options.git.clone(),
                env: self.options.git_env.clone(),
            })
            .await
            .map_err(|e| ControlError::new(e.to_string()))
        })
        .await
        .cloned()
    }

    fn get(&self, handle: &RunnerHandle) -> Result<Arc<LocalRunner>, ControlError> {
        self.runners
            .lock()
            .unwrap()
            .get(&key_of(handle))
            .cloned()
            .ok_or_else(|| ControlError::new(format!("unknown runner {}", key_of(handle))))
    }

    async fn finish(&self, runner: &LocalRunner) {
        runner.inbox.push(RunnerMessage::Stop);
        let grace = self
            .options
            .stop_grace
            .unwrap_or(Duration::from_millis(10_000));
        let done = runner.done.clone();
        tokio::select! {
            _ = done.clone() => {}
            _ = tokio::time::sleep(grace) => {
                runner.abort.cancel();
                done.await;
            }
        }
    }

    /// Worktree mode: remove the agent's worktree once its loop has ended. The branch it pushed stays on the remote.
    async fn cleanup(&self, optimization_id: &str, runner: &LocalRunner) {
        let Some(clone) = &runner.clone else { return };
        if runner.stopped.swap(true, Ordering::SeqCst) {
            return;
        }
        let queue = self.queue(optimization_id);
        let _guard = queue.lock().await;
        remove_worktree(clone, &runner.workdir).await;
    }

    /// Resolves with the loop's result when the runner finishes by itself or after `stop`.
    pub async fn result(&self, handle: &RunnerHandle) -> Result<RunnerLoopResult, ControlError> {
        Ok(self.get(handle)?.done.clone().await)
    }

    pub fn workdir_of(&self, handle: &RunnerHandle) -> Result<PathBuf, ControlError> {
        Ok(self.get(handle)?.workdir.clone())
    }

    pub async fn stop_all(&self) {
        let runners: Vec<(String, Arc<LocalRunner>)> = self
            .runners
            .lock()
            .unwrap()
            .iter()
            .map(|(k, r)| (k.clone(), r.clone()))
            .collect();
        futures::future::join_all(runners.into_iter().map(|(key, r)| async move {
            self.finish(&r).await;
            let optimization_id = key.split_once('/').map_or(key.as_str(), |(o, _)| o);
            self.cleanup(optimization_id, &r).await;
        }))
        .await;
    }
}

fn lost_result() -> RunnerLoopResult {
    RunnerLoopResult {
        reason: RunnerStopReason::Failed,
        turns: 0,
        candidates: 0,
        spend_usd: 0.0,
        best_value: 0.0,
        history: Vec::new(),
    }
}

#[async_trait]
impl RunnerControl for LocalRunnerControl {
    async fn start(
        &self,
        handle: &RunnerHandle,
        config: &RunnerConfig,
    ) -> Result<(), ControlError> {
        let key = key_of(handle);
        if self.runners.lock().unwrap().contains_key(&key) {
            return Err(ControlError::new(format!("runner {key} already started")));
        }
        let workdir = self
            .options
            .root_dir
            .join(&handle.optimization_id)
            .join(&handle.agent_id);
        let events: Arc<Mutex<Vec<AgentEvent>>> = Arc::new(Mutex::new(Vec::new()));
        let abort = CancellationToken::new();
        let inbox = Arc::new(MemoryInbox::new());
        let adapter = (self.options.adapter_for)(config, handle).map_err(ControlError::new)?;

        let mut tuning = self.options.loop_tuning.clone();
        let mut clone: Option<Git> = None;
        match self.options.workdirs {
            WorkdirMode::Worktree => {
                let c = self
                    .clone_for(&handle.optimization_id, &config.repo_url)
                    .await?;
                {
                    let queue = self.queue(&handle.optimization_id);
                    let _guard = queue.lock().await;
                    add_worktree(AddWorktreeOptions {
                        clone: c.clone(),
                        dir: workdir.clone(),
                        branch: config.branch.clone(),
                        base_ref: config.base_ref.clone(),
                        identity: Identity {
                            name: format!("{} (ratchet)", config.agent_id),
                            email: format!("{}@ratchet.local", config.agent_id),
                        },
                        git: self.options.git.clone(),
                        env: self.options.git_env.clone(),
                    })
                    .await
                    .map_err(|e| ControlError::new(e.to_string()))?;
                }
                // Refs outside HEAD are shared by every worktree of the clone: give each agent its own.
                tuning.accepted_ref = Some(format!("refs/ratchet/{}/accepted", handle.agent_id));
                tuning.best_tracking_ref = Some(format!(
                    "refs/remotes/origin/ratchet-best/{}",
                    handle.agent_id
                ));
                clone = Some(c);
            }
            WorkdirMode::Clone => {
                prepare_workdir(PrepareWorkdirOptions {
                    repo_url: config.repo_url.clone(),
                    branch: config.branch.clone(),
                    base_ref: config.base_ref.clone(),
                    dir: workdir.clone(),
                    git: self.options.git.clone(),
                    env: self.options.git_env.clone(),
                })
                .await
                .map_err(|e| ControlError::new(e.to_string()))?;
            }
        }
        if let Some(git) = &self.options.git {
            tuning.git = Some(git.clone());
        }
        if let Some(env) = &self.options.git_env {
            tuning.git_env = Some(env.clone());
        }
        if let Some(agent_env) = self
            .options
            .agent_env_for
            .as_ref()
            .and_then(|f| f(config, handle))
        {
            tuning.agent_env = Some(agent_env);
        }

        let sink_events = events.clone();
        let task = tokio::spawn(run_runner_loop(RunnerLoopOptions {
            config: config.clone(),
            adapter,
            workdir: workdir.clone(),
            inbox: inbox.clone(),
            emit: Arc::new(move |e| sink_events.lock().unwrap().push(e)),
            clock: self.clock.clone(),
            signal: Some(abort.clone()),
            tuning,
        }));
        let done: Done = async move { task.await.unwrap_or_else(|_| lost_result()) }
            .boxed()
            .shared();
        let runner = Arc::new(LocalRunner {
            inbox,
            events,
            abort,
            done,
            workdir,
            stopped: AtomicBool::new(false),
            clone,
        });
        let mut runners = self.runners.lock().unwrap();
        if runners.contains_key(&key) {
            runner.abort.cancel();
            return Err(ControlError::new(format!("runner {key} already started")));
        }
        runners.insert(key, runner);
        Ok(())
    }

    async fn send(
        &self,
        handle: &RunnerHandle,
        message: RunnerMessage,
    ) -> Result<(), ControlError> {
        self.get(handle)?.inbox.push(message);
        Ok(())
    }

    async fn poll(
        &self,
        handle: &RunnerHandle,
        cursor: Option<&str>,
    ) -> Result<PollResult, ControlError> {
        let runner = self.runners.lock().unwrap().get(&key_of(handle)).cloned();
        let Some(runner) = runner else {
            // The process restarted since this agent was launched: its loop is gone. Say so once, so the engine can end it.
            if cursor == Some("lost") {
                return Ok(PollResult {
                    events: Vec::new(),
                    cursor: "lost".to_string(),
                });
            }
            let lost = AgentEvent::new(self.clock.now(), AgentEventType::Status)
                .with_text("local agent lost: the app was restarted while it was running")
                .with_data(json!({"state": "failed", "reason": "lost"}));
            return Ok(PollResult {
                events: vec![lost],
                cursor: "lost".to_string(),
            });
        };
        let events = runner.events.lock().unwrap();
        let from: usize = cursor.and_then(|c| c.parse().ok()).unwrap_or(0);
        Ok(PollResult {
            events: events.iter().skip(from).cloned().collect(),
            cursor: events.len().to_string(),
        })
    }

    async fn stop(&self, handle: &RunnerHandle) -> Result<(), ControlError> {
        let runner = self.get(handle)?;
        self.finish(&runner).await;
        self.cleanup(&handle.optimization_id, &runner).await;
        Ok(())
    }

    /// Teardown of one optimization: stop its agents, remove their worktrees and delete the local clone. Idempotent; the commits the
    /// agents pushed are on the remote and are not touched.
    async fn release(&self, optimization_id: &str) -> Result<(), ControlError> {
        let prefix = format!("{optimization_id}/");
        let mine: Vec<Arc<LocalRunner>> = self
            .runners
            .lock()
            .unwrap()
            .iter()
            .filter(|(k, _)| k.starts_with(&prefix))
            .map(|(_, r)| r.clone())
            .collect();
        futures::future::join_all(mine.iter().map(|r| async move {
            self.finish(r).await;
            self.cleanup(optimization_id, r).await;
        }))
        .await;
        if self.options.workdirs == WorkdirMode::Worktree {
            self.clones.lock().unwrap().remove(optimization_id);
            let _ = tokio::fs::remove_dir_all(self.clone_dir(optimization_id)).await;
        }
        let _ = tokio::fs::remove_dir_all(self.options.root_dir.join(optimization_id)).await;
        Ok(())
    }
}
