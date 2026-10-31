//! Recording fakes of the product router's runtimes (feature `testing`): an engine that only records what it was asked and
//! writes the optimization rows the router looks at, and a git host with a one-file diff. Used by this crate's tests of
//! `ProductEngine` and by the daemon's tests of `RealService`.

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use ratchet_core::Calibration;
use ratchet_core::enums::{AgentsWhere, Direction, Mode};
use ratchet_core::ids::{AgentId, OptimizationId};
use ratchet_core::ports::{
    CandidateCommit, CommitFilesInput, EngineApi, EngineCreateOptimizationInput, EngineEvent,
    FileDiff, GitHost, PortError, PortResult, ProposeTemplateInput, ProposeTemplateResult,
    PullRequestInput, PullRequestRef, SandboxActivity, SandboxInput, SandboxOutputs, SandboxPlan,
    SandboxState, SandboxStateStatus, SandboxTeardownReceipt, TeardownRemoved, TeardownTrigger,
};
use ratchet_ledger::{AddAgentInput, CreateOptimizationInput as LedgerCreate, Ledger};
use tokio::sync::broadcast;

use super::product::RuntimeEngine;

const YAML: &str =
    "version: 0\nobjective:\n  name: x\n  metric: latency_ms\n  direction: minimize\n";

/// An engine that only records what it was asked and writes the optimization row the router looks at.
pub struct RecordingRuntime {
    pub name: String,
    ledger: Ledger,
    calls: Mutex<Vec<String>>,
    /// What `run_active()` answers.
    pub active: AtomicBool,
    tx: broadcast::Sender<EngineEvent>,
    pub proposals: Mutex<Vec<ProposeTemplateInput>>,
    /// Every create request seen.
    pub created: Mutex<Vec<EngineCreateOptimizationInput>>,
    seq: AtomicUsize,
    /// `propose_template` answers this when set (an `Err` is a refusal).
    pub propose: Mutex<Option<PortResult<ProposeTemplateResult>>>,
    /// `create_optimization` answers this id (without touching the ledger) when set.
    pub create_id: Mutex<Option<String>>,
    pub activity: Mutex<Option<SandboxActivity>>,
    /// `end_sandbox` answers this when set (otherwise a generic manual receipt).
    pub end_receipt: Mutex<Option<SandboxTeardownReceipt>>,
    pub receipts: Mutex<std::collections::HashMap<String, SandboxTeardownReceipt>>,
    pub calibration: Mutex<Option<Calibration>>,
    /// After `end_sandbox`, mark the sandbox destroyed in the ledger (what the engine does) ...
    pub destroy_on_end: AtomicBool,
    /// ... and answer this activity from then on.
    pub activity_after_end: Mutex<Option<SandboxActivity>>,
}

impl RecordingRuntime {
    pub fn new(name: &str, ledger: &Ledger) -> Arc<Self> {
        Arc::new(Self {
            name: name.into(),
            ledger: ledger.clone(),
            calls: Mutex::default(),
            active: AtomicBool::new(false),
            tx: broadcast::channel(64).0,
            proposals: Mutex::default(),
            created: Mutex::default(),
            seq: AtomicUsize::new(0),
            propose: Mutex::default(),
            create_id: Mutex::default(),
            activity: Mutex::default(),
            end_receipt: Mutex::default(),
            receipts: Mutex::default(),
            calibration: Mutex::default(),
            destroy_on_end: AtomicBool::new(false),
            activity_after_end: Mutex::default(),
        })
    }
    pub fn log(&self, s: String) {
        self.calls.lock().unwrap().push(s);
    }
    pub fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }
    pub fn calls_except_create(&self) -> Vec<String> {
        self.calls()
            .into_iter()
            .filter(|c| !c.starts_with("create"))
            .collect()
    }
    pub fn emit(&self, e: EngineEvent) {
        let _ = self.tx.send(e);
    }
}

#[async_trait]
impl EngineApi for RecordingRuntime {
    fn mode(&self) -> Mode {
        Mode::Live
    }
    async fn create_optimization(
        &self,
        input: &EngineCreateOptimizationInput,
    ) -> PortResult<OptimizationId> {
        self.created.lock().unwrap().push(input.clone());
        self.log(format!(
            "create:{}:{}",
            input.repo_url,
            input
                .agents_where
                .map_or("-", |w| if w == AgentsWhere::Cloud {
                    "cloud"
                } else {
                    "local"
                })
        ));
        if let Some(id) = self.create_id.lock().unwrap().clone() {
            return Ok(OptimizationId::new(id));
        }
        let n = self.seq.fetch_add(1, Ordering::SeqCst);
        let mut c = LedgerCreate::new(
            &input.repo_url,
            &input.base_ref,
            "latency_ms",
            Direction::Minimize,
            0.03,
            10.0,
            600,
            YAML,
        );
        c.name = Some(format!("{} run", self.name));
        let opt = self
            .ledger
            .create_optimization(c)
            .await
            .map_err(|e| PortError::Failed(e.to_string()))?;
        for (i, kind) in input.agents.iter().enumerate() {
            self.ledger
                .add_agent(AddAgentInput::new(
                    opt.id.clone(),
                    *kind,
                    format!("b/{}-{n}-{i}", kind.as_str()),
                ))
                .await
                .map_err(|e| PortError::Failed(e.to_string()))?;
        }
        Ok(opt.id)
    }
    async fn start(&self, id: &OptimizationId) -> PortResult<()> {
        self.log(format!("start:{id}"));
        Ok(())
    }
    async fn stop(&self, id: &OptimizationId, _reason: Option<&str>) -> PortResult<()> {
        self.log(format!("stop:{id}"));
        Ok(())
    }
    async fn pause_agent(&self, id: &AgentId) -> PortResult<()> {
        self.log(format!("pause:{id}"));
        Ok(())
    }
    async fn resume_agent(&self, id: &AgentId) -> PortResult<()> {
        self.log(format!("resume:{id}"));
        Ok(())
    }
    async fn message_agent(&self, id: &AgentId, text: &str) -> PortResult<()> {
        self.log(format!("message:{id}:{text}"));
        Ok(())
    }
    async fn open_pull_request(&self, id: &OptimizationId) -> PortResult<PullRequestRef> {
        self.log(format!("pr:{id}"));
        Ok(PullRequestRef {
            url: format!("https://github.com/x/y/pull/{}", self.name),
        })
    }
    async fn teardown(&self, id: &OptimizationId) -> PortResult<()> {
        self.log(format!("teardown:{id}"));
        Ok(())
    }
    async fn propose_template(
        &self,
        input: &ProposeTemplateInput,
    ) -> PortResult<ProposeTemplateResult> {
        self.log("propose".into());
        self.proposals.lock().unwrap().push(input.clone());
        if let Some(r) = self.propose.lock().unwrap().clone() {
            return r;
        }
        Ok(ProposeTemplateResult {
            pr_url: "https://github.com/acme/api/pull/9".into(),
            branch: "ratchet/template-x".into(),
            sha: "abc".into(),
            template_id: "http-latency".into(),
            files: vec!["ratchet.yaml".into()],
        })
    }
    async fn sandbox_state(&self, id: &str) -> PortResult<SandboxState> {
        self.log(format!("state:{id}"));
        Err(PortError::Unsupported("sandbox_state"))
    }
    async fn plan_sandbox(&self, _i: &SandboxInput) -> PortResult<SandboxPlan> {
        Err(PortError::Unsupported("plan_sandbox"))
    }
    async fn apply_sandbox(&self, _i: &SandboxInput) -> PortResult<SandboxOutputs> {
        Err(PortError::Unsupported("apply_sandbox"))
    }
    async fn destroy_sandbox(&self, _id: &str) -> PortResult<()> {
        Ok(())
    }
    fn subscribe(&self) -> broadcast::Receiver<EngineEvent> {
        self.tx.subscribe()
    }
    async fn sandbox_activity(&self, id: &str) -> PortResult<SandboxActivity> {
        self.log(format!("activity:{id}"));
        if let Some(a) = self.activity.lock().unwrap().clone() {
            return Ok(a);
        }
        Ok(SandboxActivity {
            sandbox_id: id.into(),
            status: SandboxStateStatus::Applied,
            region: Some("us-east-1".into()),
            auto: true,
            active_runs: vec![],
            instances_running: Some(0),
            spend_usd: 0.0,
            idle_since: None,
            destroy_at: None,
            last_receipt: None,
        })
    }
    async fn end_sandbox(&self, id: &str) -> PortResult<SandboxTeardownReceipt> {
        self.log(format!("end:{id}"));
        if self.destroy_on_end.load(Ordering::SeqCst) {
            let _ = self
                .ledger
                .set_sandbox_status(
                    id,
                    ratchet_core::enums::SandboxStatus::Destroyed,
                    Default::default(),
                )
                .await;
        }
        if let Some(a) = self.activity_after_end.lock().unwrap().take() {
            *self.activity.lock().unwrap() = Some(a);
        }
        if let Some(r) = self.end_receipt.lock().unwrap().clone() {
            return Ok(r);
        }
        Ok(SandboxTeardownReceipt {
            sandbox_id: id.into(),
            region: "us-east-1".into(),
            trigger: TeardownTrigger::Manual,
            created_at: None,
            destroyed_at: 1,
            runs: vec![],
            spend_usd: 0.0,
            spend_by_category: Default::default(),
            removed: TeardownRemoved {
                instances: vec![],
                agent_identities: vec![],
                secrets: vec![],
                resources: vec![],
            },
            nothing_running: true,
            orphans: vec![],
            errors: vec![],
            summary: "ok".into(),
        })
    }
    async fn teardown_receipt(
        &self,
        id: &OptimizationId,
    ) -> PortResult<Option<SandboxTeardownReceipt>> {
        self.log(format!("receipt:{id}"));
        Ok(self.receipts.lock().unwrap().get(id.as_str()).cloned())
    }
    async fn calibration(&self, id: &OptimizationId) -> PortResult<Option<Calibration>> {
        self.log(format!("calibration:{id}"));
        Ok(self.calibration.lock().unwrap().clone())
    }
    fn run_active(&self) -> bool {
        self.active.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl RuntimeEngine for RecordingRuntime {
    async fn run(&self, cancel: ratchet_core::ports::CancellationToken) {
        cancel.cancelled().await;
    }
}

/// A git host whose only job is a one-file diff.
pub struct DiffGit;

#[async_trait]
impl GitHost for DiffGit {
    async fn create_branch(&self, _: &str, _: &str) -> PortResult<()> {
        Ok(())
    }
    async fn list_candidates(&self, _: &str, _: Option<&str>) -> PortResult<Vec<CandidateCommit>> {
        Ok(vec![])
    }
    async fn changed_files(&self, _: &str, _: &str) -> PortResult<Vec<String>> {
        Ok(vec!["a.ts".into()])
    }
    async fn diff(&self, _: &str, _: &str) -> PortResult<Vec<FileDiff>> {
        Ok(vec![FileDiff {
            path: "a.ts".into(),
            additions: 1,
            deletions: 0,
            binary: false,
            patch: "@@".into(),
        }])
    }
    async fn checkout(&self, _: &str, _: &Path) -> PortResult<()> {
        Ok(())
    }
    async fn open_pull_request(&self, _: &PullRequestInput) -> PortResult<PullRequestRef> {
        Err(PortError::Unsupported("open_pull_request"))
    }
    async fn resolve_ref(&self, r: &str) -> PortResult<String> {
        Ok(r.into())
    }
    fn remote_url(&self) -> String {
        "/work/remote.git".into()
    }
    async fn commit_files(
        &self,
        _: &CommitFilesInput,
    ) -> PortResult<ratchet_core::ports::CommitRef> {
        Err(PortError::Unsupported("commit_files"))
    }
}
